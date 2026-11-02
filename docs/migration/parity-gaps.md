# Écarts de parité assumés

Divergences entre le test Ruby et le contrat observable sur le stack, retenues
exprès. Une ligne n'entre ici qu'avec une réponse réelle du stack Rails.

## Taille d'upload : 413 au bord, 422 dans le modèle

Les tests Ruby abaissent `Document::MAX_BYTE_SIZE` à 3 octets, sous la limite du
proxy, pour obtenir un 422 :

- `test/security/input_fuzz_test.rb:170` — un `Document` de 200 Mio est invalide
  (validation de modèle, pas de requête HTTP) ;
- `test/security/input_fuzz_test.rb:238` — upload synchrone rejeté en 422 ;
- `test/security/input_fuzz_test.rb:248` — upload asynchrone rejeté en 422, sans
  fichier de staging.

Sur le stack compose, `GED_MAX_UPLOAD_BYTES` (défaut 52 428 800) est à la fois
le plafond du modèle et `MAX_REQUEST_BODY` de Thruster (52_428_800 octets). Un corps plus grand
n'atteint pas Rails. Observé le 2026-11-01 avec
`GED_BACKEND=rails cargo test -p parity security` :

- statut **413**, corps **vide** ;
- aucune ligne `documents` ;
- aucune ligne `upload_requests` quand un `callback_url` est joint.

Le 422 du modèle n'est pas atteignable en boîte noire sans modifier Rails ou
baisser la limite en dessous du bord. La spec de migration retire Thruster et
fait renvoyer 413 par Axum avant toute écriture disque
(`docs/superpowers/specs/2026-11-01-rust-migration-design.md`, bord HTTP). Les
tests `security_fuzz_oversized_*` verrouillent ce contrat. L'absence du fichier
de staging reste unitaire (`rust/ged/src/storage/staging.rs`) : elle n'est pas
visible en HTTP.

Décision T2b (orchestrateur, après la revue) : le corps vide du 413 **reste
l'attente**. C'est le contrat observé sur le stack, à reproduire tel quel côté
Rust — pas un artefact de proxy à assouplir au portage. Un corps JSON
`{ "error": ... }` ou un texte du genre « length limit exceeded » serait une
divergence. L'attente reste : statut 413, corps vide (`body` de longueur 0),
aucune ligne `documents` ni `upload_requests`. Les tests
`security_fuzz_oversized_sync_upload_is_rejected_at_the_edge` et
`security_fuzz_oversized_async_upload_is_rejected_without_a_request_row` ne
changent pas.

## État JWKS de `/admin/status`

`admin_status.rs` n'impose pas une valeur unique pour la ligne « OIDC JWKS » :
elle dépend de ce que le processus a observé jusque-là. Le test admet exactement
trois états, et le port de `SystemHealth` (T10) ne doit en émettre aucun autre :

- `not fetched yet` : aucune récupération n'a encore eu lieu ;
- `keys cached` : des clés sont en cache dans leur TTL ;
- `serving stale keys` : le dernier rafraîchissement a échoué, les clés périmées
  sont servies.

Le test ne force aucun de ces états : le cache est local au processus et n'est pas
observable en boîte noire. Les transitions elles-mêmes sont vérifiées en unitaire (T5).

## Requête non analysable : un `%` invalide (proxy Thruster)

Observé le 2026-11-01 sur le stack Rails :

| Requête | Via Thruster (port publié) | Puma seul (port 3000 du conteneur) |
|---|---|---|
| `GET /up?a=%zz` | 200 | 400 |
| `GET /up?a=1&b=%zz&c=3` | 200 | 400 |
| `GET /up?a=%ff` | 400 | 400 |

Le proxy Go réécrit la requête quand elle contient un `%` mal formé : le paramètre
disparaît avant d'atteindre Rails. Thruster disparaît dans la version Rust (spec,
« Bord HTTP »), qui applique donc **la règle de Rails** : un `%` qui n'est pas suivi
de deux chiffres hexadécimaux est un 400, comme `a=%ff` (octet UTF-8 invalide).
`api_errors.rs` n'asserte pas ce cas, justement parce que la réponse dépend du
proxy. Le comportement de Rails seul est figé par le corpus
`rust/ged/tests/fixtures/rack_params_corpus.json` (généré par
`ActionDispatch::ParamBuilder`, voir `gen_rack_params_corpus.rb`).
**Changement observable au cutover.** Ce gap est un durcissement : aujourd'hui, derrière
Thruster, un client qui envoie `?a=%zz` (ou `a=%zz` dans un corps urlencoded) reçoit une
réponse normale (le paramètre est silencieusement supprimé) ; après la bascule vers le
Rust, la même requête reçoit un `400 {"error":"bad request"}`. À consigner dans le journal
de cutover et à annoncer aux applications clientes ; un client qui n'encode pas ses
valeurs (`%` littéral non échappé) casserait à ce moment-là.

Le traitement de `;` dans la requête n'a pas été observé : le port suit Rails
(`a=1;b=2` est un seul paramètre `a` valant `1;b=2`).

## Taille et nombre des paramètres

Observé le 2026-11-01 sur le stack Rails (Thruster, port publié), puis reproduit à
l'octet près par le Rust (`api_errors.rs`, `http_edge.rs`) :

| Entrée | Réponse Rails |
|---|---|
| corps urlencoded de 4 194 304 octets (4 Mio pile), clé seule ou `a=` + valeur | 401 `missing bearer token` (analysé) |
| corps urlencoded de 4 194 305 octets, 5, 8, 9 Mio | **400** `{"error":"bad request"}` (JSON sous `/api/v1`, page HTML ailleurs) |
| corps de 4096 parties séparées par `&` (clés distinctes, doublons, `a[]`) | 401 (analysé) |
| corps de 4097 parties | **400** |
| `&` × 4095 puis `&` × 4096 (parties vides comptées) | 200/401 puis **400** |
| 4097 parties séparées par `;` | 200/401 : `;` ne sépare pas, une seule partie |
| query de 10 240 octets | analysée (200 sur `/up`) |
| query de 10 241 octets, sur **n'importe quel chemin ou verbe** (même `/nope`, `POST /up`) | **400 à corps vide, sans `Content-Type`** |
| query de 5000 parties (`a&a&…`, < 10 Kio) | 200 : pas de plafond de parties sur la query |
| corps **JSON** de 9 Mio | 401 (analysé à la demande, voir plus bas) |

Les plafonds du corps sont les défauts de Rack 3.2 (`bytesize_limit` 4 Mio,
`params_limit` 4096 ; le décompte est celui de `&`), non surchargés dans l'app. Le
plafond de la query n'est **pas** celui de Rack : c'est la limite de ligne de requête
de Puma (10 Kio), qui répond avant Rails, d'où la réponse vide ; l'ancienne observation
« 5000 clés en query → 400 » venait de cette limite d'octets, pas d'un compte de clés.
Le Rust applique les trois : `FORM_BYTES_LIMIT`, `FORM_PARTS_LIMIT` et
`QUERY_BYTES_LIMIT` (`rust/ged/src/http/params.rs`), le dernier dans la couche la plus
externe (avant le routage). Le corps urlencoded n'est lu que si une route correspond
(404 sur un chemin sans route, quelle que soit sa taille) ; un corps que le handler n'a
pas lu est vidé avant de répondre (jusqu'à `GED_MAX_UPLOAD_BYTES` octets / 5 s : c'est le `MAX_REQUEST_BODY` du proxy, au-delà la requête est de toute façon refusée en 413), comme le fait le proxy, pour ne
pas réinitialiser la connexion d'un client en train d'envoyer.

Écarts restants, volontairement non reproduits : (a) un corps **JSON** est plafonné à
8 Mio côté Rust (`JSON_BODY_LIMIT`) alors que Rails n'en a pas ; (b) les autres limites
de Puma/Thruster sur la ligne de requête (chemin au-delà de 8192 octets → 400 vide ;
en-têtes au-delà d'~1 Mio → 431 de Thruster) ne sont pas répliquées.

## Analyse des paramètres : ce qui est eager, ce qui est lazy

Observé sur le stack Rails (Thruster inclus) :

- La **requête** et un corps **urlencoded** sont analysés avant le contrôleur, sur
  toute route qui correspond : un conflit de types (`a=1&a[b]=2`), une profondeur
  de 100 ou plus, un octet invalide donnent un 400 `{"error":"bad request"}` (JSON
  sous `/api/v1`) *avant* l'authentification, y compris sur `/up`. Sur un chemin
  sans route, c'est le 404 qui gagne. De même sur une route existante dont le verbe n'est pas servi
  (`POST /up?a=%ff`) : 404, jamais 400 (observé sur Rails, figé par
  `api_errors_a_verb_the_route_does_not_serve_is_a_404_even_with_an_unparsable_query`).
- Un corps **JSON** est analysé **à la demande** : `POST /api/v1/documents` avec
  `{bad` et sans jeton répond 401 `missing bearer token`, pas 400. Le 400 n'arrive
  qu'une fois authentifié, quand le contrôleur lit les paramètres. Le Rust a la même
  forme : le garde de routeur ne lit que la requête et les formulaires ; le JSON
  est analysé par l'extracteur `RequestParams`, que les handlers appellent après
  l'authentification (T5/T7).
- Un verbe que la route ne sert pas (`POST /up`, `PATCH /api/v1/departments`) est un
  404 (erreur de routage), jamais un 405.

Écarts connus du Rust par rapport à Rails sur le JSON, non couverts par le corpus :
les entiers de plus de 64 bits sont lus comme flottants (Rails garde un `Integer`
exact) ; aucun champ de l'API n'attend un tel nombre. Les commentaires `/* … */` et
`// …` sont acceptés comme en Ruby (observé). La profondeur d'imbrication maximale
est 100, comme `JSON.parse`. Les corps `multipart` ne sont pas analysés par le garde
(T7, avec l'upload).

## Sérialisation JSON

Fixée en T4 et vérifiée sur le stack Rails :

- dates : `YYYY-MM-DDTHH:MM:SS.mmmZ`, trois décimales tronquées (jamais arrondies),
  toujours `Z` (`Time.utc(2026,11,1,21,12,39,123999).as_json` donne `…39.123Z`) ;
- `<`, `>`, `&`, U+2028 et U+2029 sont échappés en `\u003c`, `\u003e`, `\u0026`,
  `\u2028`, `\u2029` dans les chaînes (`{error: "a<b>&c"}.to_json`) ;
- l'ordre des clés est l'ordre d'insertion.

## Obfuscation SQL des spans (helper Ruby, pas le stack HTTP)

Comparé le 2026-11-02 à `OpenTelemetry::Helpers::SqlProcessor.obfuscate_sql`
(`adapter: :postgres`) du gem `opentelemetry-helpers-sql-processor 0.5.0`
épinglé dans `Gemfile.lock`, le même helper que
`config/initializers/opentelemetry.rb` branche pour `db_statement: :obfuscate`.
Les chaînes golden de `rust/ged/tests/telemetry.rs`, y compris les formes
`\'`, donnent les mêmes sorties. Trois écarts restent, tous hors de ces
goldens. Le Rust masque au moins autant : un littéral que Ruby cache ne
reste pas en clair.

`.*?` des dollar-quotes Ruby ne traverse pas un retour à la ligne, donc le
texte qui suit fuit. Le Rust masque toute la quote jusqu'au fermant.

| SQL | Ruby | Rust |
|---|---|---|
| `SELECT $$a<NL>secret$$ FROM t` | `SELECT ?<NL>secret?` (`secret` en clair) | `SELECT ? FROM t` |

Un commentaire de ligne Ruby s'arrête avant `\r` (`(?=\r|\n|$)`). La regex
Rust `(?:#|--).*` s'arrête au `\n` et avale le `\r`.

| SQL | Ruby | Rust |
|---|---|---|
| `SELECT 1 -- secret<CR><NL>FROM t` | `SELECT ? ?<CR><NL>FROM t` | `SELECT ? ?<NL>FROM t` |
| `SELECT 1 -- x<CR>FROM t` | `SELECT ? ?<CR>FROM t` | `SELECT ? ?` |

Une chaîne non UTF-8 n'atteint pas l'obfuscateur Rust (`&str`). Ruby passe
d'abord par `OpenTelemetry::Common::Utilities.utf8_encode`. La limite des
2 000 caractères est la même (caractères, pas octets).

## Claims du JWT : sémantique du gem `jwt` 3.2 reproduite (T5)

`rust/ged/src/auth/token_validator.rs` signe/vérifie avec `jsonwebtoken` (RS256
seul) et refait les contrôles de claims à la main, comme `jwt` 3.2 (lu dans le
gem installé, pas supposé) :

- aucun leeway ; `exp <= maintenant` est expiré, `nbf > maintenant` est prématuré ;
- `exp` et `nbf` ne sont contrôlés **que s'ils sont présents** : un token sans
  `exp` est accepté par Rails et l'est donc côté Rust (comportement discutable,
  reproduit tel quel, comme l'impose la spec) ; une valeur non numérique est lue
  comme `String#to_i` (`"abc"`, `null` → 0, donc expiré). Les entiers
  hors `i64` (`"99999999999999999999"`, `"9_9999999999"`, flottant `1e30`) sont des
  bignums côté Ruby : le Rust **sature** à `i64::MAX`/`i64::MIN` au lieu de
  retomber sur 0, donc un `nbf` inreprésentable est rejeté (`ImmatureSignature`
  sur Rails, jamais lu comme « déjà valide ») et un `exp` inreprésentable reste
  valable, exactement comme Rails. Les particularités de `to_i` (un `_` isolé
  entre chiffres, préfixe `0d`, espaces ASCII seulement) sont reproduites.
  Ces cas sont couverts par `auth_validator.rs` et comparés en direct sur les
  deux backends par `parity/tests/auth_matrix.rs` ;
- `aud` : intersection non vide entre le claim (chaîne ou tableau) et
  `GED_OIDC_AUDIENCE` ; absent ou vide → refus ;
- un token sans `kid` est refusé ; un `kid` inconnu force un seul rafraîchissement
  par fenêtre de 30 s (rotation de clés).

Non couvert en comparaison live (`auth_matrix`, stack de parité configuré en
binding `audience` et `GED_CLIENT_ID_CLAIM` par défaut) : le binding `scope`
(`GED_OIDC_REQUIRED_SCOPE`, claims `scope`/`scp`) et `GED_CLIENT_ID_CLAIM`
personnalisé, ainsi que `nbf == now` (trop fragile en live). Ces trois points
sont couverts côté Rust seul par `auth_validator.rs` et lus sur le code Rails.

Écarts connus, volontaires, tous plus stricts que Rails (fail-closed) :

- `exp` ou `nbf` booléen, tableau ou objet, ou payload JSON qui n'est pas un
  objet : Rails lève `NoMethodError`/`TypeError` (500, sondé sur `jwt` 3.2.0) ;
  le Rust répond 401 `invalid token` pour chacune de ces formes, `exp` comme
  `nbf` (`check_time` rejette tout `exp`/`nbf` qui n'est ni nombre, ni chaîne,
  ni `null`, au lieu de le lire comme `0`, ce qui aurait accepté un `nbf`). Le
  500 n'est pas reproduit, volontairement (pas de cas dans `auth_matrix`, qui
  n'asserte que des comportements identiques) ; couvert par les tests de
  `token_validator.rs` et `auth_validator.rs`.
- claim d'identifiant `true` : Rails le passe à `find_by` (403 `client not
  registered`, ou correspondance avec un client nommé `"t"`) ; le Rust refuse en
  401. Un tableau non vide : Rails fait un `IN` et peut authentifier le premier
  client existant ; le Rust refuse en 401.
- variable de configuration présente mais **vide** (`GED_OIDC_AUDIENCE=""`) :
  traitée comme absente (401) côté Rust, alors que `ENV.fetch` Rails renverrait
  `""` et attendrait un `aud` vide.

Autres écarts connus, volontaires : une variable de configuration absente
(`GED_OIDC_ISSUER`, `GED_OIDC_AUDIENCE`, `GED_OIDC_REQUIRED_SCOPE`) donne un 401
`invalid token` côté Rust, là où Rails lève `KeyError` (500) ; en production le
boot est de toute façon refusé dans ce cas. Un claim d'identifiant numérique est
converti en chaîne ; tableau, objet, booléen ou `null` sont refusés (Rails
passerait un tableau non vide à `find_by`).

Départements en corbeille et permissions : comme Rails (relations à
`default_scope`), la remontée des ancêtres s'arrête au premier ancêtre en
corbeille — aucun grant au-dessus ne s'applique — et un grant sur un
département en corbeille n'accorde rien (`rust/ged/tests/auth_access.rs`).

## Stockage : type détecté, noms de fichiers (T6)

- `documents.content_type` garde le type **déclaré** par le client, tel quel (Rails
  ne le réécrit pas) ; le type **détecté par le contenu** (Marcel, comme Active
  Storage) est enregistré sur le fichier (`stored_files.content_type`). Le port de
  `detect_content_type` tourne sur les tables de Marcel 1.2.1 générées
  mécaniquement (`gen_marcel_tables.rb`) ; 110 réponses relevées sur Rails sont
  figées dans `rust/ged/tests/fixtures/content_type_corpus.json`.
- `Content-Disposition` : mêmes en-têtes que `send_file` (`ActionDispatch::Http::ContentDisposition`,
  translittération `I18n`), figés sur 61 noms hostiles (`filename_corpus.json`).
- **Écart (durcissement)** : dans les entrées ZIP de l'admin, Rails écrit
  `"#{id}-#{filename}"` tel quel ; un nom stocké réduit à `/` (seul cas où la
  normalisation laisse un séparateur) donnerait une entrée-répertoire. Le Rust
  remplace `/`, `\\` et les caractères de contrôle par `_` dans ces entrées. Aucun
  autre nom ne change.
- Création d'un document : le fichier est écrit sur disque **avant** la
  transaction (Rails l'écrit au commit). Un rollback ou une validation en échec
  laisse donc un fichier que l'appelant doit supprimer ; un crash peut laisser un
  fichier orphelin, jamais une ligne sans fichier.
