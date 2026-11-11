# Écarts de parité assumés

Divergences entre le test Ruby et le contrat observable sur le stack, retenues
exprès. Une ligne n'entre ici qu'avec une réponse réelle du stack Rails.
Les mesures ci-dessous restent le journal. Le dépôt courant ne contient plus
Rails : la suite de parité parle à Rust seul.

## Rejouer Rails depuis le tag `rails-final`

Le tag local `rails-final` pointe sur `aba3d4b` (pas poussé). C'est le dernier
arbre où `GED_STACK=rails` et l'adaptateur `rust/parity/src/rails.rs` existent.
La parité Rails y avait été mesurée en vague 8 : 320 passed, 0 failed. Elle
n'a pas été relancée au moment de la suppression.

Pour rejouer une comparaison :

```sh
git worktree add ../ged-rails-final rails-final
cd ../ged-rails-final
# cet arbre a encore GED_STACK=rails et bin/rails
bin/worktree-stack up          # avec GED_STACK=rails, ports isolés
```

Depuis cet arbre-ci (Rust), `diff_fuzz` compare deux URLs. Il n'importe pas
l'adaptateur Rails. Campagnes finales, déjà enregistrées plus bas (2026-11-11) :
seed 13, 100000 requêtes, `documented=1769`, `undocumented=0`, 233 s ; seed 29,
100000 requêtes, `documented=1604`, `undocumented=0`, 236 s. Pour les refaire,
construire le binaire dans `rust/` et passer `--rails-url` / `--rails-db` de la
worktree `rails-final` avec `--rust-url` / `--rust-db` de la stack Rust. Le
golden `rust/parity/fixtures/rails_business_schema.sql` se régénère en
dumpant les tables métier de cette stack Rails, pas depuis ce dépôt.

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

### `%` mal formé dans le chemin

Observé le 2026-11-11, socket brut, sur le stack Rails (`ged-t13-rails`, port
publié 3140 = Thruster, Puma sur le port 3000 du conteneur) et sur le Rust
(`ged-rust-t13-fuzz`, port 3040). Le fuzzer (reqwest) a vu la même paire pour
`GET /api/v1/documents/%zz?a=%2` : Thruster `400` `400 Bad Request`, Rust
`400` `{"error":"bad request"}`.

| Requête | Thruster (port publié) | Puma :3000 | Rust |
|---|---|---|---|
| `GET /api/v1/documents/%zz` | 400 `text/plain`, corps `400 Bad Request`, sans `Content-Length` ni en-tête de sécurité | 401 `missing bearer token` | 401, identique |
| `GET /api/v1/documents/%zz?a=%2` (aussi `%`, `%zz`, `%2G`) | même 400 texte | 400 `{"error":"bad request"}`, sans en-tête de sécurité | 400, identique |
| `GET /api/v1/documents/%zz?a=%ff` | même 400 texte | 400 `{"error":"bad request"}` | 400, identique |
| `GET /api/v1/documents/%zz?limit=1` | même 400 texte | 401 `missing bearer token` | 401, identique |
| `GET /api/v1/documents/%25zz` | 401 (le `%` est bien formé) | 401 | 401 |
| `GET /up/%zz`, `GET /api/v1/ping/%zz` | 400 texte | 404 de la route | 404, identique |
| `POST /api/v1/documents/%zz` corps `a=%zz` | 400 texte | 404 (verbe sans route, corps non lu) | 404, identique |

Thruster rejette le chemin avant Rails. Puma accepte le chemin. Le Rust suit
Puma, comme pour le `%` de query : la spec retire Thruster. Ce n'est pas le
gap de réécriture de paramètre. L'allowlist ne le classe que si les trois
conditions tiennent : le chemin contient un `%` qui n'est pas deux chiffres
hexadécimaux, la réponse Rails est exactement ce 400 texte (un seul en-tête
de contrat, `content-type: text/plain; charset=utf-8`), et la réponse Rust est
exactement `400` `{"error":"bad request"}` avec `content-type` JSON,
`content-length: 23` et aucun autre en-tête de contrat. En plus, la query ou
le corps urlencoded doit lui-même être rejeté par Puma ( `%` mal formé, ou
`%HH` qui n'est pas de l'UTF-8, comme `%ff`). Un chemin seul, un 401 ou un
404 restent un échec.

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

## Délais HTTP : en-têtes, corps, connexion inactive (revue finale M2)

Rails tournait derrière Thruster 0.1.23 (`Dockerfile.rails`), qui est un `net/http` de
Go. Valeurs lues dans les sources, version épinglée (`rails-final:Gemfile.lock`) :

| Couche | Réglage | Défaut | Source |
|---|---|---|---|
| Thruster 0.1.23 | `HTTP_READ_TIMEOUT` (`ReadTimeout` Go : **toute la requête**, en-têtes et corps) | 30 s | `internal/config.go` (`defaultHttpReadTimeout`), appliqué dans `internal/server.go` |
| Thruster 0.1.23 | `HTTP_WRITE_TIMEOUT` | 30 s | idem |
| Thruster 0.1.23 | `HTTP_IDLE_TIMEOUT` (keep-alive) | 60 s | idem |
| Puma 8.0.2 | `first_data_timeout` | 30 s | `lib/puma/configuration.rb:144` |
| Puma 8.0.2 | `persistent_timeout` | 65 s | `lib/puma/configuration.rb:163` |

`rails-final:config/puma.rb` ne fixe aucun délai : ce sont les défauts. Le Rust,
qui retire Thruster, pose les siens dans `rust/ged/src/http/serve.rs`
(`Timeouts::default()`), testés par `rust/ged/tests/http_timeouts.rs` :

- **En-têtes : 30 s** (timer hyper, `header_read_timeout`), aussi pour attendre la
  requête suivante sur une connexion maintenue. Avant : aucun délai, une requête aux
  en-têtes incomplets pendant 75 s était servie (mesuré par la revue).
- **Corps : 30 s d'inactivité entre deux morceaux** (`IdleBody`). Le `.part` d'un
  upload abandonné est supprimé quand la requête est interrompue (test
  `an_abandoned_upload_leaves_no_part_file`).

Écarts restants, volontaires :

- **Pas d'échéance totale sur le corps.** Thruster coupe la requête à 30 s au total,
  donc un upload de 50 Mio plus lent qu'environ 14 Mbit/s échouait sous Rails. Le Rust
  accepte un gros upload lent tant qu'il progresse (au moins un morceau par 30 s) : un
  client lent mais légitime passe, un client à débit nul est coupé. Conséquence :
  un client qui envoie un octet par 29 s peut tenir une connexion ; son coût est borné
  par la limite d'upload (`GED_MAX_UPLOAD_BYTES`) pour le disque, et par 4 Mio pour
  les parties texte (M1). Un proxy frontal avec échéance totale reste possible.
- **Pas de délai d'écriture** (`HTTP_WRITE_TIMEOUT`) : un client qui lit sa réponse
  lentement (ZIP, téléchargement) n'est pas coupé. Un téléchargement légitime de
  plusieurs Gio dépasserait 30 s.
- **Pas de plafond de connexions simultanées** (non vérifié côté Thruster).

## Concurrence des vérifications argon2 (revue finale m1)

La connexion admin lance un argon2id par tentative sur le pool bloquant. Au plus
`ARGON2_CONCURRENCY` (4) vérifications tournent à la fois (sémaphore global,
`admin/session.rs`) ; les autres attendent un permis. Le throttle par client reste
10 tentatives par 3 minutes. Rails/bcrypt était borné par les threads Puma (3).
Écart : une rafale de tentatives de clés de throttle différentes est ralentie, pas
refusée ; le coût en attente est une tâche parquée par tentative.

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
- `render json:` (Rails 8.1, `escape_json_responses = false`) n'échappe pas
  `<`, `>`, `&`, U+2028 ni U+2029 (`to_json(escape: false)`, observé le
  2026-11-11 : `POST /api/v1/departments` renvoie `{"name":"a<b>&c-…"}`).
  `{}.to_json` sans option (exceptions app) échappe encore `<`, `>`, `&`, pas
  U+2028/U+2029 ; ses messages sont des phrases de statut fixes, donc les
  octets coincident avec l'encodeur de `render json:` ;
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

## File de jobs : process mort, worker purgé, kind inconnu (T8)

Comportement de Solid Queue 1.5.0 relu dans le gem
(`app/models/solid_queue/process/prunable.rb`, `claimed_execution.rb`) :

- **Un process mort n'est pas rejoué.** `Process#prune` appelle
  `fail_all_claimed_executions_with(ProcessPrunedError)` : chaque exécution
  réclamée devient une `FailedExecution`, sans retry automatique. Seul un arrêt
  propre (`ClaimedExecution#release`) remet le job en file. Le Rust fait pareil :
  `queue::recover_abandoned` marque `failed_at` (avec `last_error` « Process pruned… »)
  et **garde** l'attempt. Un job qui tue son process ne boucle donc pas ; la
  réparation des uploads revient à `ReconcileUploadRequestsJob`, comme en Rails.
  Seul `release_claims` (arrêt propre après `shutdown_timeout`) décrémente `attempts`.
- **Callbacks : au moins une fois (revue finale m5), écart voulu par rapport à Solid
  Queue.** Trois trous faisaient perdre un callback : (1) l'enregistrement du résultat
  (`complete`/`fail`) qui échoue sur une panne de base laissait le claim à un worker
  vivant, jamais repris ; (2) l'arrêt propre ne relâchait les claims qu'après un
  dépassement du `shutdown_timeout` ; (3) un worker mort en pleine livraison voyait son
  job marqué `failed`. Le Rust réessaie l'enregistrement (4 reprises sur 12 s), relâche
  toujours les claims à l'arrêt, et `recover_abandoned_requeuing` remet en file, due
  tout de suite, les claims d'un worker mort pour les kinds `Job::at_least_once`
  (aujourd'hui `DeliverCallbackJob` seul, tant que `attempts < 5`). Le destinataire
  peut donc recevoir un callback deux fois (même `request_id`). `StoreDocumentJob`
  reste « exactement une fois » : un process mort n'y est toujours pas rejoué,
  réparé par la réconciliation. Il reste une panne de base plus longue que 12 s :
  le claim est alors recouvré comme ci-dessus à l'expiration du heartbeat.
- **Un runner qui s'arrête arrête `ged worker` (revue finale m6).** Si un pool se
  termine hors arrêt demandé (par exemple `heartbeat::register` échoue sur une
  panne de base au démarrage), le processus arrête les autres, sort en erreur, et
  Compose le relance (`restart: on-failure` sur `worker`). Un arrêt demandé (SIGTERM)
  sort en 0 et n'est pas relancé.
- **Seuils configurables comme Solid Queue** : `SOLID_QUEUE_PROCESS_HEARTBEAT_INTERVAL`
  (60 s) et `SOLID_QUEUE_PROCESS_ALIVE_THRESHOLD` (300 s), en secondes. Un intervalle
  qui n'est pas inférieur au seuil est ignoré (valeurs par défaut).

Écarts assumés :

- **Worker vivant mais purgé** (process figé, partition vers Postgres > seuil).
  Solid Queue ne traite pas ce cas. Ici le beat suivant ré-enregistre le worker sous
  le même id et **annule** les jobs en cours (leur claim a déjà été échouée par la
  purge) ; `claim` refuse un worker non enregistré. Aucune double exécution : le job
  purgé est échoué, jamais relancé. Le job en cours peut avoir produit des effets de
  bord avant l'annulation (point d'await) : les handlers T9 doivent rester idempotents.
- **Kind inconnu** (déploiement progressif) : Solid Queue échoue le job
  (`NameError` à la désérialisation). Le Rust le **réessaie** avec le backoff
  polynomial, au plus `UNKNOWN_KIND_MAX_ATTEMPTS` (8) fois, puis l'échoue.
- **Claim vide sous contention** : en READ COMMITTED, une ligne modifiée entre le
  choix du sous-select et son verrou peut donner 0 ligne alors que d'autres sont
  prêtes. Coût : un `polling_interval` de retard, jamais de double claim. Non corrigé.
- Index `index_jobs_ready (queue, id) INCLUDE (run_at)` : les retries futurs sont
  écartés sans visiter le tas ; le tri reste `ORDER BY id`. À réévaluer en T15 si la file
  grossit.

## API documents, départements, uploads (T7)

- **Répertoire de stockage :** `storage/` (`storage::STORAGE_DIR`), relatif au
  répertoire courant. L'image a pour répertoire de travail `/rails` et le
  volume compose est monté sur `/rails/storage`. Le processus HTTP, le worker
  et le contrôle disque de `/admin/status` utilisent cette constante. Aucune
  variable d'environnement ne la déplace. Le staging est
  `storage/staging/<id>`.
- **Limite de taille :** `GED_MAX_UPLOAD_BYTES` s'applique au **corps entier** de la
  requête (comme `MAX_REQUEST_BODY` de Thruster), pas au seul fichier : un fichier
  d'exactement la limite est refusé (413 à corps vide, avant l'authentification quand
  `Content-Length` l'annonce, sinon pendant la lecture). Un corps multipart cassé
  est un 400 avant l'authentification, comme Rack.
- **Upload async :** le fichier est d'abord écrit dans le stockage (le corps est
  analysé avant de savoir si `callback_url` est présent), puis **renommé** vers le
  staging dans la transaction qui insère la requête et met le job en file : aucun
  fichier de staging sans ligne, aucune ligne sans job.
- **Documents d'un département en corbeille :** Rails lève un 500 (`nil.self_and_ancestors`)
  sur show/download/delete d'un document vivant dont le département est en corbeille
  (état normalement inatteignable) ; le Rust répond 403 (fail-closed).
- **`updated_at` d'un document créé :** Rails le fait avancer de quelques ms après la
  création (attache Active Storage) ; le Rust le laisse égal à `created_at`.
- **Parties texte multipart :** plafonnées à 4 Mio au total (400 au-delà) ; Rails n'a
  pas ce plafond. Un fichier envoyé sous un autre nom que `file` est ignoré.
  **Mémoire (revue finale M1) :** une partie texte est lue morceau par morceau et la
  lecture s'arrête dès que le cumul dépasse 4 Mio ; le reste de la partie n'est ni
  tiré de la connexion ni gardé (test `an_oversized_text_part_stops_the_read_at_the_form_limit`).
  Avant, chaque partie texte était bufferisée en entier (jusqu'à `GED_MAX_UPLOAD_BYTES`)
  avant le contrôle, avant l'authentification.
- **Coût CPU du nettoyeur multipart (revue finale m12) :** tant qu'un en-tête de
  partie n'est pas fini, chaque poussée relit le tampon (jusqu'à 64 Kio). Mesuré avant
  correctif : 8,3 s de CPU (release) pour 64 Kio d'en-tête envoyés un octet à la fois.
  Les morceaux de moins de 4 Kio sont maintenant regroupés tant qu'un en-tête est en
  cours (`GATHER_BYTES`), soit une vingtaine de poussées au lieu de 65 000 (test
  `a_header_sent_byte_by_byte_costs_few_scrubber_pushes`). Le regroupement ne touche
  jamais un corps de fichier.
- **Limite de taille sur tous les endpoints :** le 413 vide s'applique à toute
  requête (`PUT …/tags`, `DELETE …/bulk_destroy`, `POST /departments`, …) : par
  `Content-Length` avant tout routage et authentification, et pour un corps sans
  longueur, dès que le handler en a lu plus que la limite. `docs/api/openapi.yaml` le
  documente (`components/responses/PayloadTooLarge`).
- **Fichiers orphelins après un crash (upload async) :** un `kill -9`/OOM entre
  l'écriture du fichier (`storage/xx/yy/<key>` puis `staging/<id>`) et le `COMMIT`
  laisse un fichier sans ligne. Rails a la même fenêtre (staging écrit après
  `create!`, enqueue après). **Staging : traité par T9** (ajout par rapport à
  Rails, qui ne purge le staging qu'après 30 jours) : `ReconcileUploadRequestsJob`
  supprime `staging/<uuid>` et `staging/<uuid>.part` de plus d'un jour
  (`ORPHAN_STAGING_AGE`) dont aucune requête `pending`/`processing` n'est propriétaire
  (orphelin de crash, ou requête terminale dont le nettoyage a échoué) ; le staging
  d'une requête vivante n'est jamais supprimé, les autres noms gardent la règle des
  30 jours. **Reste ouvert :** `storage/**` sans `stored_files` (relève de la purge
  quotidienne, hors de ce balayage).
- **Zone id IPv6 dans `callback_url` :** `[fe80::1%25eth0]` est refusé (`Invalid`) à la
  requête par le Rust ; Ruby le lit comme un nom, accepté à la requête et refusé à la
  livraison. Direction plus sûre.
- **Faiblesses héritées de `CallbackUrlPolicy` (non corrigées, par fidélité) :** `::`,
  `::127.0.0.1` (IPv4-compatible) et NAT64 `64:ff9b::/96` ne sont pas bloqués, ni en Rails
  ni en Rust. À signaler à l'audit (`docs/security/`).
- **`CallbackUrlPolicy` :** à la requête, `999.1.1.1` n'est pas une adresse (aucun
  encodage `inet_aton`) : traité comme un nom, accepté (202, observé sur Rails) ; la
  livraison le refuse (non résolu). Le pinning lui-même est dans T9.

## Pipeline async, callbacks, réconciliation, purge (T9)

Écarts assumés, tous du côté de la robustesse (aucune attente de parité n'est
affaiblie : messages d'erreur, payload du callback et statuts sont ceux de Rails) :

- **Callback enqueué dans la transaction du document.** Rails met le
  `DeliverCallbackJob` en file dans le `ensure`, après la transaction : un crash
  entre les deux perd la notification. `StoreDocumentJob` commit ici le document,
  son `stored_files`, `completed` **et** l'enqueue du callback en une transaction
  (idem pour `failed`). La réconciliation n'a donc plus de notification à
  rattraper.
- **Exécution dépassée par la réconciliation.** Une exécution lente (> `STUCK_AFTER`)
  dont la requête a été remise en `pending` par `ReconcileUploadRequestsJob`
  n'écrit rien (`UPDATE … WHERE status = 'processing'` sans ligne : rollback du
  document, suppression du fichier stocké, fichier de staging laissé à la nouvelle
  exécution). Rails stockait un second document.
- **Orphelins : balayage dans la purge quotidienne.** Rails supprime les blobs Active
  Storage non rattachés (> 1 jour) à chaque réconciliation (10 min). Ici il n'y a pas
  de table de blobs : `PurgeDeletedDocumentsJob` parcourt le répertoire de stockage
  une fois par jour et supprime les fichiers sans ligne `stored_files` et les `.part`
  abandonnés, plus vieux d'un jour (mtime). Le répertoire `staging/` reste à la
  réconciliation (30 jours).
- **Purge isolée par élément.** Rails (`find_each(&:destroy!)`) abandonne toute la passe
  sur la première erreur de document ; ici chaque document et chaque département est
  purgé séparément, l'erreur est loggée et le job termine en échec (`Retry`, une seule
  tentative) après avoir traité tout le reste. La suppression d'un document est
  gardée par `purge_at <= now` dans le `DELETE` lui-même : un document restauré entre
  la liste et la purge survit.
- **`ged.job.duration`** est enregistré par chaque job (`timed`) et non par un
  abonné `perform.active_job` ; `ged.documents.uploaded` est émis après le commit
  (Rails : avant, dans `DocumentCreator`), donc jamais pour un document annulé.
- **Hook d'épuisement.** `Job::exhausted` (ajout au trait de T8, appelé par le
  pool au dernier `Retry`) porte le `retry_on … do |job, error|` de Rails : métrique
  `exhausted` et journal d'erreur.
- **Livraison HTTP.** `reqwest` : un client par livraison avec l'adresse épinglée via
  `resolve(host, ip:port)` (Host, SNI et certificat gardent le nom d'hôte), ni proxy
  ni redirection suivie (un 3xx est un échec comme en `Net::HTTP`), délais 5 s
  connexion / 10 s lecture. Un hôte littéral IP ne peut pas être épinglé : l'adresse que `reqwest` composerait (parsing WHATWG) est comparée strictement à celle validée par la policy, et toute différence (encodage décimal/hexa/octal/court, IPv6, IPv4-mappé) est refusée sans retry (testé).
- **Livraison : pas d'échéance totale (revue finale m4, hérité de Rails, non changé).**
  Le délai de 10 s s'applique à chaque lecture, pas à la livraison entière, comme
  `Net::HTTP#read_timeout`. Un destinataire qui égrène sa réponse octet par octet
  occupe un des deux threads du pool `default` aussi longtemps qu'il envoie. Un
  `.timeout()` total sur le client le règlerait ; il est laissé pour fidélité.
- **Message d'erreur d'un fichier de staging manquant** : reproduit le texte Ruby
  (`No such file or directory @ rb_sysopen - <chemin>`), avec le chemin du staging Rust.

Branché à l'intégration (vague 6, M4) :

- Le worker et `ged run-job` enregistrent `DeliverCallbackJob` avec `SsrfPolicy`
  (`jobs::deliver_callback::production_policy()`), adaptateur de
  `policy::callback_url::decide(url, true)` : résolution unique, adresse approuvée
  épinglée, refus = `refused`, jamais de retry. Le stub `PolicyNotWired` est supprimé.
- Cas SSRF `999.1.1.1` et `[::1]` (report de la vague 3) : ils relèvent de la policy
  (T7), pas du job ; le job ne fait que consommer la décision.
- `ged run-job <Kind> [json]` (= `Job.perform_now`) alimente `Backend::run_job` du
  backend Rust des tests de parité.

## Admin : session, CSRF, en-têtes (T10)

Relevé sur le stack Rails le 2026-11-03, reproduit par `rust/ged/src/admin/`.

- **Session** : cookie `_ged_session` signé (HMAC-SHA256, clé dérivée de
  `SECRET_KEY_BASE`), non chiffré, `path=/; httponly; samesite=lax`. Rails le chiffre ;
  seul le nom et les attributs sont observables. Un changement de mot de passe
  invalide la session (comme Devise).
- **Pas de CSP** : l'initialiseur Rails est entièrement commenté, donc aucun en-tête
  `Content-Security-Policy` ni balise meta sur l'admin. Le Rust n'en émet pas non
  plus (la CSP de l'aperçu de documents relève de T11). En-têtes par défaut posés sur
  toute réponse : `X-Frame-Options: SAMEORIGIN`, `X-Content-Type-Options: nosniff`,
  `X-XSS-Protection: 0`, `X-Permitted-Cross-Domain-Policies: none`,
  `Referrer-Policy: strict-origin-when-cross-origin`.
- **Durcissements** : la session est renouvelée (nouveau secret CSRF) à la connexion
  et à la déconnexion ; un nom d'utilisateur inconnu coûte autant qu'un mauvais mot
  de passe (hash leurre) ; une création concurrente d'un même nom de département ou
  de client donne le 422 « has already been taken » au lieu d'un 500.
- **Limite de connexions** : 10 par 3 minutes et par client, clé = adresse du pair TCP
  (`ConnectInfo`). `X-Forwarded-For` n'est lu que si le pair est un proxy de confiance
  (loopback, privé, link-local : la liste par défaut de `ActionDispatch::RemoteIp`) ;
  le client est alors l'adresse la plus à droite qui n'est pas un proxy de confiance
  (sinon la plus à gauche, sinon le pair), comme `request.remote_ip`. **Écart
  (durcissement)** : Rails croit aussi l'en-tête d'un pair public, donc un service
  exposé en direct s'y contourne avec un en-tête forgé ; le Rust ignore l'en-tête dans
  ce cas. `Client-Ip` n'est pas lu (Rails lève `IpSpoofAttackError` s'il contredit
  `X-Forwarded-For`). Derrière un proxy privé qui n'envoie pas `X-Forwarded-For`, tous
  les clients partagent le seau du proxy, comme Rails. La table est plafonnée à
  10 000 clients (éviction du plus ancien) ; le hash argon2 tourne hors du runtime
  (`spawn_blocking`) et le hash leurre est calculé au démarrage (échec fatal).
- **CSRF et multipart** : comme Rails, toute requête mutante (multipart compris) exige
  un jeton valide (champ `authenticity_token` ou `X-CSRF-Token`) et un `Origin`
  identique, sinon 422. Seul `POST /admin/documents` lit son jeton dans le handler
  (le champ peut suivre le fichier) ; son `Origin` est vérifié par le garde.
- **Mots de passe (M1, cutover)** : Devise stocke du bcrypt ; le Rust ne vérifie que
  l'argon2 (PHC) et **ne reprend pas les hashes bcrypt** (décision du propriétaire :
  aucune donnée réelle à migrer). Un hash bcrypt donne un 422 à la connexion. Étape
  obligatoire de la bascule : `ged seed` (réécrit le hash de l'admin de
  `GED_ADMIN_USERNAME`/`GED_ADMIN_PASSWORD` en argon2). Tout autre admin créé hors seed
  doit être recréé. Testé : `a_devise_bcrypt_hash_is_refused_until_the_seed_rewrites_it`.
- **500 reproduits** : parent de département inexistant (`parent_id=999999` ou
  `abc`), `department=x` (scalaire à la place d'un hash), mise à la corbeille d'un
  département qui a des enfants. `x=1` sans `department` donne 400.
- **Flash** : un message survit aux requêtes qui ne font que rediriger
  (`/admin` → `/admin/documents`), comme Rails.
- **Documents admin (T11)** : `documents.rs` et `trash.rs` remplacent le stub.
  L'upload lit le jeton avant le fichier quand celui-ci le précède, et envoie les
  octets vers le stockage du processus (`storage/`, un chunk à la fois, pas un
  `Vec`). Un corps plus grand que `GED_MAX_UPLOAD_BYTES` répond 413. Le nom de
  fichier passe par `normalize_filename`. L'`Origin` reste vérifié par le garde.
  Les fichiers statiques n'ont pas les en-têtes de sécurité des contrôleurs et
  portent `Cache-Control: public, max-age=31556952`. Le contrôle disque de
  `/admin/status` lit le même répertoire `storage/` que les octets uploadés
  (montage compose `/rails/storage`).
- **Identifiants qui ne sont pas des UUID** (édition, téléchargement, aperçu,
  suppression, restauration, et `GET /admin/documents/:id`, qui n'a pas de
  route `show`) : Rails caste l'uuid invalide en nil et répond **404** avec
  `public/404.html` (« The page you were looking for doesn't exist »). Ce
  n'est pas un 500. Une sélection en lot vide ou faite de blancs redirige
  avec l'alerte « No documents selected. ». Une sélection non vide dont aucun
  id n'est un UUID lance quand même l'action : « 0 documents moved to trash. »
  ou un ZIP vide (`application/zip`). Les ids valides d'un mélange sont
  appliqués. Couvert par `admin_documents.rs` et `admin_trash.rs`.

## T13 — allowlist, familles non générées, notes de comparaison

Le fuzzer ne classe un `%` mal formé de query ou de corps urlencoded comme le
gap Thruster que si les trois conditions tiennent : le `%` n'est pas dans le
chemin, Rust répond exactement `400` avec le corps `{"error":"bad request"}`,
et la requête réécrite comme Thruster (le paramètre fautif supprimé) rejouée
sur Rust est égale à la réponse Rails (statut, corps normalisé, en-têtes de
contrat). Un `%` dans le chemin est un gap séparé, décrit plus haut
(« `%` mal formé dans le chemin ») : Thruster répond `400` texte, et le Rust
n'est allowlisté que lorsqu'il suit Puma avec le `400` JSON non analysable.

Familles non générées, et pourquoi :

- **Scope** (`GED_OIDC_TOKEN_BINDING=scope`). Les deux stacks démarrent en
  mode audience. `sign` ne pose pas de claim `scope`. Exercer le mode scope
  demanderait un second boot, pas une requête de plus.
- **413**. Chaque corps ferait 50 Mio + 1. La campagne ne l'envoie pas. Le
  bord est couvert par `security_fuzz_oversized_*`.
- **Corbeille**. Pas de route API. La restauration est l'UI admin. Un document
  dont le département est en corbeille est déjà un écart (Rails 500, Rust 403,
  plus haut) : le générateur ne le produit pas.
- **Fin d'upload asynchrone**. La campagne arrête les workers. `upload-async`
  compare le `202` (callback `https://example.com/cb`). La transition
  `completed` et le POST de callback dépendent du worker.
- **Instantané SQL**. Le harnais compare les réponses HTTP, pas les tables.

`multipart-edge` envoie 129 parties texte. Rack 3.2 compte les noms de
fichier vers `multipart_file_limit` (128 : la 128e partie avec un `filename`,
même vide, lève `MultipartPartLimitError`) et toutes les parties vers
`multipart_total_part_limit` (4096). Les parties texte en dessous sont des
paramètres. Observé le 2026-11-11 sur la stack Rails : 129 parties texte
arrivent au contrôleur (`department_id` manquant, en-têtes de sécurité) ;
127 fichiers aussi ; la 128e partie fichier et la 4096e partie, quelle
qu'elle soit, sortent de l'application et Thruster répond 500, corps vide,
sans content-type. Le Rust fait de même. Un `%` dans le chemin est le gap
séparé décrit plus haut, pas celui-ci.

Familles ajoutées au générateur, déterministes : `upload-async` (202, workers
arrêtés), `bulk-over-max` (501 ids, mélange lecture seule + valide, id déjà en
corbeille + valide), `multipart-edge` (sans boundary, tronqué, LF seul, 129
parties, 4097 parties de formulaire), `auth-unknown` (`fuzz-unknown`),
`auth-nogrant` (`fuzz-nogrant`, aucun grant), `auth-other` (`fuzz-other`,
lecture et écriture sur le département 1, `uploaded_by`), `head` sur `/ping`,
`/documents`, `/departments` et un document. Le nom de fichier peut contenir
un CR (`line\rN.txt`) en plus du LF.

### Tri des départements

`GET /api/v1/departments` suit `order(:name)` Rails. Pas de départage par
`id` : un `ORDER BY id` en plus séparait deux départements homonymes dans un
ordre que Rails ne garantit pas (observé le 2026-11-11, deux « Fuzz Payroll »).
L'unicité du nom est par `parent_id`. Avec `LIMIT`/`OFFSET`, deux noms égaux
peuvent changer de page selon le plan, des deux côtés. Le fuzzer ne traite
pas cet ordre comme un écart à corriger.

### Ce que la normalisation ne prouve pas

Les UUID et les dates `YYYY-MM-DDTHH:MM:SS.mmmZ` sont numérotés dans l'ordre
d'apparition. Une valeur fausse à la même position reste invisible. L'ETag est
réduit à sa forme faible ou forte. Une date sans heure (`2026-01-02`,
`2026-11-01Z`) n'est pas ce motif : Ruby 3.4.10 lève `ArgumentError`, et le
Rust la refuse aussi. `HEAD` sur une réponse de contrôleur envoie un corps
vide et `Content-Length: 0` (observé sur `HEAD /api/v1/ping` et `HEAD /up`).
Un `Content-Type` vide devient `text/html; charset=utf-8`, sauf 1xx, 204, 205
et 304.

### Campagne

Deux runs le 2026-11-11, workers arrêtés, binaire `rust/target/release/diff_fuzz`
reconstruit après l'allowlist du `%` de chemin. Aucune ligne `body:` : les
corps urlencoded générés n'ont pas divergé. Les tables ci-dessous sont la
sortie réelle, progressions intermédiaires omises.

Le libellé `path:/api/v1/documents/%zz` sans query dans ces tables est la
requête `…/%zz?a=%ff`. `%ff` est un `%HH` bien formé qui n'est pas de l'UTF-8,
donc l'ancien `documented_form` ne l'imprimait pas et la ligne ressemblait à
un chemin seul. Le libellé inclut maintenant cette query (`query:a=%ff`).
Les sorties ci-dessous sont celles du run, non réécrites.

```text
cd rust && caffeinate -i ./target/release/diff_fuzz \
  --rails-url http://localhost:3140 --rust-url http://localhost:3040 \
  --rails-db postgres://postgres:postgres@localhost:5572/ged_production \
  --rust-db postgres://postgres:postgres@localhost:5472/ged_production \
  --oauth-url http://localhost:9332 --signing-key /tmp/t13-signing_key.pem \
  --issuer http://oauth-provider:9292 --audience http://web:80 \
  --seed 13 --count 100000 --report /tmp/t13-campaign-13.txt
```

```text
diff_fuzz seed=13 count=100000 rails=http://localhost:3140 rust=http://localhost:3040 issuer=http://oauth-provider:9292
documented index=77 Thruster supprime un % mal formé ; Rust répond 400 {"error":"bad request"} (parity-gaps.md, requête non analysable) GET /api/v1/documents?a=% auth=Valid body=none
documented index=165 Thruster rejette un % mal formé dans le chemin (400 text/plain) ; Rust suit Puma et répond 400 {"error":"bad request"} (parity-gaps.md, % dans le chemin) GET /api/v1/documents/%zz?a=%2 auth=Valid body=none
distribution:
  path:/api/v1/documents/%zz GET rails=400 rust=400 count=68
  path:/api/v1/documents/%zz query:a=% GET rails=400 rust=400 count=58
  path:/api/v1/documents/%zz query:a=%2 GET rails=400 rust=400 count=49
  path:/api/v1/documents/%zz query:a=%2G GET rails=400 rust=400 count=63
  path:/api/v1/documents/%zz query:a=%zz GET rails=400 rust=400 count=54
  path:/api/v1/documents/%zz query:a=%zz&limit=1 GET rails=400 rust=400 count=49
  path:/api/v1/documents/%zz query:a=1&b=%zz&c=3 GET rails=400 rust=400 count=53
  path:/api/v1/documents/%zz query:limit=%zz GET rails=400 rust=400 count=62
  path:/api/v1/documents/%zz query:tag=%zz GET rails=400 rust=400 count=53
  query:a=% GET rails=200 rust=400 count=139
  query:a=%2 GET rails=200 rust=400 count=163
  query:a=%2G GET rails=200 rust=400 count=167
  query:a=%zz GET rails=200 rust=400 count=165
  query:a=%zz&limit=1 GET rails=200 rust=400 count=170
  query:a=1&b=%zz&c=3 GET rails=200 rust=400 count=155
  query:limit=%zz GET rails=200 rust=400 count=154
  query:tag=%zz GET rails=200 rust=400 count=147
seed=13 elapsed=233s requests=100000 documented=1769 undocumented=0
result=ok requests=100000 documented=1769 undocumented=0
```

Même commande avec `--seed 29 --report /tmp/t13-campaign-29.txt` :

```text
diff_fuzz seed=29 count=100000 rails=http://localhost:3140 rust=http://localhost:3040 issuer=http://oauth-provider:9292
documented index=57 Thruster supprime un % mal formé ; Rust répond 400 {"error":"bad request"} (parity-gaps.md, requête non analysable) GET /api/v1/documents?limit=%zz auth=Valid body=none
documented index=162 Thruster rejette un % mal formé dans le chemin (400 text/plain) ; Rust suit Puma et répond 400 {"error":"bad request"} (parity-gaps.md, % dans le chemin) GET /api/v1/documents/%zz?a=%2G auth=Valid body=none
distribution:
  path:/api/v1/documents/%zz GET rails=400 rust=400 count=53
  path:/api/v1/documents/%zz query:a=% GET rails=400 rust=400 count=53
  path:/api/v1/documents/%zz query:a=%2 GET rails=400 rust=400 count=43
  path:/api/v1/documents/%zz query:a=%2G GET rails=400 rust=400 count=51
  path:/api/v1/documents/%zz query:a=%zz GET rails=400 rust=400 count=53
  path:/api/v1/documents/%zz query:a=%zz&limit=1 GET rails=400 rust=400 count=50
  path:/api/v1/documents/%zz query:a=1&b=%zz&c=3 GET rails=400 rust=400 count=44
  path:/api/v1/documents/%zz query:limit=%zz GET rails=400 rust=400 count=52
  path:/api/v1/documents/%zz query:tag=%zz GET rails=400 rust=400 count=48
  query:a=% GET rails=200 rust=400 count=143
  query:a=%2 GET rails=200 rust=400 count=134
  query:a=%2G GET rails=200 rust=400 count=147
  query:a=%zz GET rails=200 rust=400 count=154
  query:a=%zz&limit=1 GET rails=200 rust=400 count=145
  query:a=1&b=%zz&c=3 GET rails=200 rust=400 count=148
  query:limit=%zz GET rails=200 rust=400 count=144
  query:tag=%zz GET rails=200 rust=400 count=142
seed=29 elapsed=236s requests=100000 documented=1604 undocumented=0
result=ok requests=100000 documented=1604 undocumented=0
```
