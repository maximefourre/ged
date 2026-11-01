# Migration GED de Rails vers Rust — design

Date : 2026-11-01. Statut : validé par le propriétaire le 2026-11-01. Remplace, une fois livré, les parties
Rails de `docs/superpowers/specs/2026-07-08-ged-design.md` (le comportement
reste identique, seule l'implémentation change).

## Intention

Remplacer l'application Rails par une application Rust au comportement
identique. C'est un **exercice voulu par le propriétaire du projet**, avec les
performances comme bonus. Aucun problème de performance mesuré ne le motive : les
campagnes de juillet 2026 (`docs/perf/`) montrent >10 000 docs/min à 3 workers et
aucune perte sous crash. La réussite se juge donc sur la parité prouvée, pas sur un
gain de perf.

## Objectifs

- L'app Rust couvre tout ce que fait Rails aujourd'hui : API `/api/v1`, admin web,
  jobs, observabilité.
- La parité est **prouvée** par une suite boîte noire validée d'abord contre Rails.
- Rails est supprimé du repo à la fin (code, Gemfile, tests Ruby de l'app).
- Les décisions sont enregistrées en ADR dans codebase memory, et les workers lisent
  cette base avant chaque tâche.

## Non-objectifs

- Aucune nouvelle fonctionnalité, aucun correctif de comportement opportuniste. Un
  comportement Rails jugé discutable (ex. F2 de `docs/perf/2026-07-18-soak.md`,
  `limit[]=1` → 200) est reproduit tel quel et listé dans un rapport d'écarts.
- Pas de reprise de données : aucune donnée réelle ni client réel n'existe. Les
  mots de passe, fichiers et lignes existants en local ne sont pas conservés.
- Pas de coexistence Rails/Rust en production, pas de bascule progressive.
- Pas de portage de `oauth-provider/` (Sinatra, outil de dev temporaire : il reste
  tel quel) ni de `bin/perf-upload` et `bin/monkey` (Ruby stdlib, ils pilotent l'app
  en HTTP et restent nos outils de mesure).
- Les non-objectifs du spec d'origine restent valables (recherche plein texte,
  versioning, sauvegardes automatiques).

## Anti-objectifs (échec même si « ça marche »)

- Une garantie affaiblie sans que ce soit dit : frontière de permissions,
  read ≠ write, SSRF avec pinning d'IP, exactly-once à la reprise, fail-closed sur
  l'auth, streaming ZIP sans bufferisation complète.
- Des tests adaptés au Rust au lieu du Rust adapté aux tests. Un test de parité ne
  change qu'avec une preuve que Rails se comporte aussi comme le nouveau test.
- Un `/admin` visuellement différent : mêmes libellés, mêmes champs, même CSS.

## Contraintes

- PostgreSQL 17, Docker Compose, modèle de stack par worktree (`bin/worktree-stack`)
  et isolation des ports conservés.
- Contrat HTTP inchangé : routes, codes, corps JSON, format d'erreurs,
  `docs/api/openapi.yaml` servi à l'identique.
- Mêmes variables d'environnement `GED_*`, `JOB_*`, `OTEL_*`. `SECRET_KEY_BASE`
  reste le nom de la clé de session pour limiter les changements de compose.
- Mêmes noms de métriques OpenTelemetry, avec des attributs à valeurs fermées.
  OTel reste complètement inactif sans `OTEL_EXPORTER_OTLP_ENDPOINT`.
- Règles du repo (`AGENTS.md`) : worktree dédié, historique linéaire, aucun commit
  ou merge sans demande explicite, docs synchronisées.

## Stack (décidée avec le propriétaire)

| Rôle | Choix |
|---|---|
| Web et API | Axum (tokio) |
| Base de données | SQLx, requêtes vérifiées à la compilation, cache `.sqlx` committé |
| Vues admin | Askama, CSS et JS actuels repris tels quels |
| Jobs | File maison sur PostgreSQL (`FOR UPDATE SKIP LOCKED`) |
| Fichiers | Stockage disque maison |
| Login admin | Sessions signées + argon2 |
| JWT | `jsonwebtoken` |

## Conception

### Organisation

Workspace Cargo à la racine du repo, dans `rust/` pendant le chantier puis conservé
à cet emplacement. Un binaire `ged` avec les sous-commandes `serve`, `worker`,
`migrate`, `seed`, plus des bibliothèques internes dont le découpage est laissé au
builder. Une seule image Docker, comme aujourd'hui, pour `web` et `worker`.

### Schéma

- Les tables métier gardent leurs colonnes, types, contraintes et index :
  `admins`, `app_clients`, `client_department_accesses`, `departments`, `documents`,
  `upload_requests`. Migrations SQLx avec un schéma initial unique.
- `documents` et `upload_requests` restent en clé uuid, le reste en bigint.
- `active_storage_*` est remplacé par une table `stored_files` (ou des colonnes sur
  `documents` : au choix du builder) contenant clé, nom, type, taille, checksum.
- `solid_queue_*` est remplacé par les tables de la file maison, dans la **même
  base** que les données métier : c'est ce qui permet la transaction unique.

### File de jobs

- Enqueue transactionnel avec les écritures métier.
- Deux pools isolés : `uploads` (`JOB_THREADS` × `JOB_CONCURRENCY`) et `default`
  (2 workers fixes). Pas de pool global : un backlog `uploads` ne doit jamais
  affamer les callbacks ni la réconciliation.
- Pause de la file `uploads` stockée en base et respectée par toutes les réplicas.
- Heartbeat des workers, lu par `/admin/status`.
- Claim abandonné par un worker mort récupéré par heartbeat expiré, en plus de
  `ReconcileUploadRequestsJob` (`STUCK_AFTER` = 600 s) qui reste porté tel quel.
- Tâches planifiées dédupliquées par `(task_key, run_at)` avec plusieurs réplicas :
  réconciliation toutes les 10 min, purge à 3 h, nettoyage des jobs terminés.
- Arrêt propre sous 30 s (`SOLID_QUEUE_SHUTDOWN_TIMEOUT` conserve son nom et son
  rôle), `stop_grace_period` à 40 s.
- Retries de `DeliverCallbackJob` : 5 tentatives, backoff polynomial.

### Bord HTTP

Thruster disparaît. Axum assure directement la limite `GED_MAX_UPLOAD_BYTES` (413
avant écriture disque), le streaming des téléchargements et le service des assets.
Le streaming ZIP passe par une bibliothèque asynchrone, entrée par entrée. Mêmes
en-têtes de sécurité et même CSP que Rails.

### Sémantique à préserver à l'identique

Sources de référence : `AGENTS.md` (sections architecture et implémentation) et le
code Ruby. Les points qui ne doivent pas être réinterprétés :

- autorisation `mode` obligatoire, mode inconnu impossible (type Rust fermé) ;
- `JwksCache` : TTL 300 s, clés périmées servies sur échec, fail-closed si jamais
  récupérées, `health` sans appel HTTP ;
- `GED_OIDC_TOKEN_BINDING` audience/scope, refus de démarrer en production si la
  configuration manque ;
- SSRF : refus à la requête avec normalisation `inet_aton` indépendante de la libc,
  puis résolution unique et pinning de l'IP à la livraison en conservant Host/SNI ;
- soft delete document et département, restauration, purge gated sur les références
  vues sans filtre, `purge_at` identique en lot ;
- suppression en lot fail-closed, 500 ids max ;
- pagination avec `total` calculé avant limit/offset, `MAX_OFFSET` ;
- erreurs JSON sur toutes les erreurs `/api/v1`, y compris JSON malformé.

## Preuve de parité

Trois couches, toutes conservées après la suppression de Rails.

1. **Suite boîte noire** (crate `rust/parity`, `cargo test`). Elle parle HTTP à une
   URL fournie par `GED_BASE_URL` et à la base via `DATABASE_URL`. Elle couvre l'API,
   la matrice auth, les permissions, l'injection de paramètres, les parcours admin
   (formulaires HTML, session, CSRF), le soft delete et la purge. Les 404 tests Ruby
   servent d'inventaire : chaque test observable par HTTP y a un équivalent.
   Pour les comportements dépendant du temps, un adaptateur par backend avance
   `purge_at` en base puis déclenche le job (`bin/rails runner` contre Rails,
   `ged` contre Rust). **Elle doit passer intégralement contre Rails avant la
   première ligne de portage**, puis le même code doit passer contre Rust.
2. **Tests unitaires Rust**, portés depuis les tests Ruby, pour ce qu'on ne voit pas
   en HTTP : pinning DNS, cache JWKS, crash entre les deux écritures de
   `StoreDocumentJob`, couverture des queues.
3. **Tests navigateur** Playwright pour le JS de l'admin (dropzone, multi-sélection,
   thème), qui remplacent `test/system/admin_ui_test.rb`. Fermeture explicite du
   navigateur en `finally`.

Validation finale sur stack Docker Rust : `bin/perf-upload --profile
mix|many-small|few-large`, `--chaos` web et worker, `bin/monkey`. Attendu : zéro
perte, zéro doublon et aucun 5xx. Les résultats sont publiés dans `docs/perf/` avec
les chiffres réels comparés à juillet, sur la même machine, sans seuil de débit
imposé. Toute régression est rapportée, pas masquée.

## Documentation

- `AGENTS.md`, `docs/architecture/c4.md`, `bin/ci`, `bin/worktree-stack`, Dockerfile
  et compose sont réécrits pour Rust. Les vérifications de `c4_diagrams_test.rb`,
  `api_docs_test.rb` et `queue_coverage_test.rb` sont portées.
- ADR dans codebase memory : décisions de stack, file de jobs, stockage, preuve de
  parité. Le projet est réindexé après chaque vague pour que les workers lisent
  l'état courant.

## Livraison

Séquence (le découpage fin est laissé au plan) : outillage Rust → suite de parité
verte contre Rails → socle (config, schéma, auth) → API → file et jobs → admin →
observabilité → bascule Docker et CI → suppression de Rails → validation perf et
chaos → stack isolée pour validation par le propriétaire avant tout merge.

Rails reste dans la branche jusqu'à l'étape de suppression : c'est la référence de
la parité. Le Rust n'atteint `main` qu'en une fois, une fois la suppression faite.

## Laissé au builder

Découpage interne des crates, choix des petites bibliothèques (ZIP, sessions, OTel),
forme exacte des tables de file et de stockage, disposition des clés sur disque,
organisation de la suite de parité.

## Arbitrages après relecture builder

- **Messages d'erreur (décision du propriétaire) :** au caractère près sur l'API,
  y compris ceux générés par Rails et leur ordre. Sur l'admin, seuls le statut et les
  libellés visibles sont vérifiés.
- **Oracle de parité :** la suite est l'inventaire des particularités de parsing
  Rack/Rails. Elle est complétée par une phase de fuzz différentiel : mêmes requêtes
  envoyées à Rails et à Rust, réponses comparées.
- **`content_type` :** détection par le contenu avec la même priorité qu'Active
  Storage/Marcel, et mêmes règles de service inline ou en pièce jointe. Le contrôle
  de sécurité des aperçus (SVG, HTML) doit rester identique.
- **Temps et concurrence :** pas de comparaison instant par instant avec Rails. Les
  critères sont les garanties énoncées (exactly-once documents, at-least-once
  callbacks, 5 tentatives), prouvées par tests unitaires et `--chaos`.
- **Fixtures :** la suite peut écrire directement en SQL dans les tables métier, qui
  restent identiques entre Rails et Rust. Elle n'accède jamais aux tables de stockage
  ni de file, qui diffèrent : les fichiers passent toujours par HTTP.
- **Harnais :** stack compose du worktree complète. La suite enregistre elle-même ses
  clients auprès de `oauth-provider`, et se connecte à l'admin avec `GED_ADMIN_*`.
- **Assets :** mêmes fichiers source et même rendu. Les URLs n'ont pas à être
  identiques ; empreinte et cache long sont conservés si les tests l'exigent.
- **CI et déploiement :** `bin/ci`, Dockerfile et workflow GitHub existant sont portés.
  Le déploiement Render reste hors périmètre.

## Calls de l'orchestrateur à vérifier

- Workers : Sonnet 5.5 effort medium pour les tâches sensibles (auth, permissions,
  SSRF, file de jobs). Grok 4.7 pour les tâches cadrées (vues admin, portage de
  tests), en effort high par décision du propriétaire, sous réserve de confirmer son
  lancement par Orca sur la première tâche.
- `rust/` plutôt que la racine, pour garder Rails intact pendant le chantier.
- Suite de parité écrite en Rust pour survivre à la suppression de Ruby.
