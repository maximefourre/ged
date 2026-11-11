# Migration GED Rails vers Rust — plan d'implémentation

> **Pour les workers :** ce plan est exécuté par des workers Orca supervisés. Chaque
> tâche est livrée en spec autonome avec `worker-start`. Les étapes s'écrivent en TDD,
> avec `superpowers:test-driven-development`. Cocher les cases au fil de l'avancement.

**Goal :** remplacer Rails par une app Rust au comportement identique, prouvé par une
suite de parité validée d'abord contre Rails.

**Architecture :** workspace Cargo sous `rust/`, un binaire `ged` (`serve`, `worker`,
`migrate`, `seed`) et une crate `parity` en boîte noire. Rails reste la référence
dans la branche jusqu'à sa suppression en fin de chantier.

**Tech Stack :** Rust stable (via mise), Axum, SQLx, Askama, `jsonwebtoken`, argon2,
PostgreSQL 17, Playwright pour le navigateur.

**Spec :** `docs/superpowers/specs/2026-11-01-rust-migration-design.md`

## Global Constraints

- Le comportement HTTP vient de Rails, pas d'une interprétation : en cas de doute,
  interroger le stack Rails et ajouter un test de parité.
- Messages d'erreur API au caractère près, y compris ceux générés par Rails et leur
  ordre. Admin : statut et libellés visibles.
- Dates JSON au format exact de Rails. Le fixer dans la tâche T4, une fois pour toutes.
- Tables métier identiques à `db/schema.rb` : colonnes, types, contraintes, index.
- Mêmes variables `GED_*`, `JOB_*`, `OTEL_*`, `SOLID_QUEUE_SHUTDOWN_TIMEOUT`,
  `SECRET_KEY_BASE`.
- Un test de parité ne change qu'avec une preuve que Rails se comporte comme le nouveau
  test. Toute divergence assumée va dans `docs/migration/parity-gaps.md`.
- Aucun `unsafe`, sauf justification écrite. Aucun `unwrap` sur une entrée externe.
- `cargo fmt --check`, `cargo clippy -- -D warnings` et `cargo test -p ged` verts
  à chaque tâche. `bin/ci` lance cette même cible, puis `cargo test -p parity`
  seulement quand la pile du worktree répond (T14). `bin/ci` Rails reste vert
  tant que Rails existe.
- Navigateurs headless toujours fermés en `finally`. Vérifier
  `pgrep -fl -- '--headless'` en fin de tâche.
- Avant chaque tâche : lire les ADR et le projet `ged-migration-rust` dans codebase
  memory, puis `AGENTS.md` et la spec.
- Jamais de chiffres de perf écrits avant la fin réelle d'un run.

## Review Focus

1. Noms de fichiers hostiles (Unicode, `../`, retours ligne, guillemets) dans
   `Content-Disposition` et les entrées ZIP : pas d'injection d'en-tête ni de sortie
   de répertoire. Tests en T6 et T11.
2. Upload coupé en plein corps ou trop gros : 413 ou erreur propre, sans fichier de
   staging orphelin ni ligne fantôme. Test en T7.
3. Deux réplicas sur le même job, ou un worker tué en plein claim : un seul effet,
   reprise garantie. Test en T8.
4. Ancêtre mis à la corbeille entre grant et accès, ou arbre profond : aucune
   escalade ni 500. Test en T5.
5. Création concurrente d'un même département : une 201 et des 422, jamais de violation
   d'unicité en 500. Test en T7.

## Structure

```
rust/Cargo.toml                 workspace
rust/ged/src/main.rs            sous-commandes
rust/ged/src/config.rs          configuration, refus de boot fail-closed
rust/ged/src/http/              routeur, erreurs, paramètres, sécurité
rust/ged/src/api/               /api/v1
rust/ged/src/admin/             sessions, CSRF, écrans
rust/ged/src/domain/            documents, départements, clients, tags
rust/ged/src/auth/              token_validator, jwks_cache
rust/ged/src/storage/           disque, staging, détection de type
rust/ged/src/jobs/              file, pools, planificateur, jobs
rust/ged/src/policy/            callback_url
rust/ged/src/telemetry.rs       OTel
rust/ged/migrations/            SQLx
rust/ged/templates/             Askama
rust/parity/                    suite boîte noire, adaptateurs Rails et Rust
rust/e2e/                       Playwright
docs/migration/parity-inventory.md  test Ruby → test de parité/unitaire
docs/migration/parity-gaps.md       écarts assumés
```

Interfaces transverses, à conserver telles quelles :

```rust
pub enum AccessMode { Read, Write }            // fermé : pas de mode inconnu
AppClient::can_access(&self, db, dept_id: i64, mode: AccessMode) -> Result<bool>
AppClient::accessible_department_ids(&self, db, mode: AccessMode) -> Result<Vec<i64>>
DocumentCreator::create(tx: &mut PgTx, input: NewDocument) -> Result<Document>
queue::enqueue(tx: &mut PgTx, job: JobPayload, queue: QueueName) -> Result<JobId>
enum QueueName { Uploads, Default }
Backend (parity) : base_url(), db(), run_job(name), reset()
```

## Exécution

- **Branche d'intégration :** `maximefourre/orchestrate-delegate-workers`.
- **Isolation :** un worktree enfant Orca par worker, chacun avec sa stack. Le worker
  commite dans son worktree. L'orchestrateur rebase sur la branche d'intégration après
  vérification. Aucun push, aucun merge vers `main`.
- **Vérification :** à chaque tâche, un worker relecteur indépendant Sonnet relit le
  diff contre la spec. L'orchestrateur exécute lui-même les commandes d'acceptation.
- **Échec :** rapport à l'utilisateur, pas de relance sur un autre modèle.
- **Modèles :** S = Sonnet 5.5 effort medium. G = Grok 4.7 effort high.
- **Fin de vague :** réindexation de `ged-migration-rust` dans codebase memory.

| Vague | Tâches | Dépend de |
|---|---|---|
| 1 | T0 | — |
| 2 | T1 | T0 |
| 3 | T2a API, T2b sécurité, T3a admin HTTP, T3b Playwright (parallèles) | T1 |
| 4 | T4 | T2, T3 |
| 5 | T5, T6, T8, T12 (parallèles) | T4 |
| 6 | T7, T9 (parallèles), T10 | T5, T6, T8 |
| 7 | T11, T14 (parallèles) | T7, T9, T10 |
| 8 | T13, T15 (parallèles) | T11, T14 |
| 9 | T16 | T13, T15 |
| 10 | Revue finale (Fable 5.2) | T16 intégré |

T9 dans la vague 6 ne dépend que de `DocumentCreator` et de la file ; son acceptation
complète (parité `api_async_*`) se vérifie après intégration de T7. T15 et T13
utilisent chacun leur stack : leurs mesures de perf ne doivent pas tourner en même
temps qu'un autre run lourd, donc T15 mesure seul sur la machine une fois T13 fini ou
mis en pause.

## Tâches

### T0 — Outillage Rust (S)

**Files :** `.mise.toml`, `rust/Cargo.toml`, `rust/ged/src/main.rs`.
**Produces :** binaire `ged`, route `GET /up`.

- [ ] Ajouter `rust` à `.mise.toml` et l'installer.
- [ ] Écrire le test `up_returns_200`, le voir échouer, puis l'implémenter.
- [ ] Ajouter `fmt`, `clippy` et `test` à `bin/ci`.
- [ ] Vérifier : `cargo test` et `bin/ci` verts.

### T1 — Harnais de parité et matrice auth (S)

**Files :** `rust/parity/`, `docs/migration/parity-inventory.md`.
**Produces :** trait `Backend`, adaptateur Rails, fixtures SQL, enregistrement de
clients auprès de `oauth-provider`, utilitaires JSON et session admin.

- [ ] Inventorier les 404 tests Ruby : parité, unitaire Rust ou navigateur.
- [ ] Porter `test/security/auth_matrix_test.rb` comme premier module.
- [ ] Vérifier : `GED_BACKEND=rails cargo test -p parity auth` vert contre la stack.

### T2 — Parité API (G)

**Files :** `rust/parity/tests/api_*.rs`, `security_*.rs`.

- [ ] Couvrir `test/controllers/api`, `authz_idor`, `input_fuzz`, `ssrf` (partie
  requête), soft delete, purge, messages exacts.
- [ ] Vérifier : suite verte contre Rails et inventaire à jour. Ce test confirme aussi
  le lancement de Grok par Orca.

### T3 — Parité admin et navigateur (G)

**Files :** `rust/parity/tests/admin_*.rs`, `rust/e2e/`.

- [ ] Porter `test/controllers/admin` en parcours HTTP.
- [ ] Porter `test/system/admin_ui_test.rb` en Playwright.
- [ ] Vérifier : les deux suites vertes contre Rails, aucun navigateur restant.

### T4 — Socle (S)

**Files :** `config.rs`, `http/`, `migrations/`, `domain/`.
**Produces :** `AppState`, `ApiError`, extracteur `RackParams`, sérialisation des
dates, `ged migrate`, `ged seed`.

- [ ] Tests d'abord : refus de boot en production (audience, scope), parsing imbriqué
  (`tags[type]=x`, `limit[]=1`, doublons, vide ou absent), JSON malformé en 400, 404
  et erreurs internes JSON sous `/api/v1`.
- [ ] Migrations : comparer `pg_dump --schema-only` des tables métier avec Rails.
- [ ] Vérifier : `parity_schema` vert, parité `/up` et erreurs verte contre Rust.

### T5 — Authentification et permissions (S)

**Files :** `auth/`, `domain/app_client.rs`, `domain/department.rs`.
**Produces :** `AuthenticatedClient`, `can_access`, `accessible_department_ids`.

- [ ] Tests unitaires portés : cache JWKS (TTL 300 s, périmé servi, fail-closed,
  `health` sans HTTP), binding audience/scope, `GED_CLIENT_ID_CLAIM`.
- [ ] Test Review Focus 4 : ancêtre en corbeille, arbre profond.
- [ ] Vérifier : parité `auth` verte contre Rust.

### T6 — Stockage et création de documents (S)

**Files :** `storage/`, `domain/document.rs`, `domain/document_creator.rs`,
`domain/tags.rs`.
**Produces :** `DocumentCreator::create`, `Storage::{put, open, delete}`,
`StagingStore`, `detect_content_type`.

- [ ] Figer par tests contre Rails le type détecté : PDF, PNG, SVG, HTML, ZIP/Office,
  binaire, extension trompeuse.
- [ ] Porter `KvTaggable`, `metadata_hash`, soft delete et restauration.
- [ ] Test Review Focus 1 : noms de fichiers hostiles.
- [ ] Vérifier : tests unitaires verts, résultats de détection identiques à Rails.

### T7 — API complète (S)

**Files :** `api/`.

- [ ] Documents, tags, départements, polling, ping, suppression en lot, téléchargement
  en streaming, OpenAPI et Redoc, limite de taille, en-têtes.
- [ ] Tests Review Focus 2 et 5.
- [ ] Vérifier : parité API verte sauf `api_async_*`, que T9 active.

### T8 — File de jobs (S)

**Files :** `jobs/queue.rs`, `jobs/pool.rs`, `jobs/scheduler.rs`,
`jobs/heartbeat.rs`, `migrations/`.
**Produces :** `queue::enqueue`, `queue::pause`, `queue::resume`, `queue::is_paused`,
`ged worker`, `Job` trait.

- [ ] Tests d'abord : enqueue annulé avec la transaction, deux pools isolés sous
  backlog, pause entre réplicas, tâche planifiée exécutée une seule fois avec trois
  ordonnanceurs, arrêt propre sous `SOLID_QUEUE_SHUTDOWN_TIMEOUT`, retries.
- [ ] Test Review Focus 3 : deux réplicas, worker tué en plein claim.
- [ ] Vérifier : tests verts répétés 20 fois sans échec intermittent.

### T9 — Pipeline async, SSRF et purge (S)

**Files :** `jobs/store_document.rs`, `deliver_callback.rs`, `reconcile.rs`,
`purge.rs`, `policy/callback_url.rs`.

- [ ] Porter `CallbackUrlPolicy` : `inet_aton` maison et plages bloquées.
- [ ] Test : résolution unique et pinning de l'IP, Host et SNI conservés.
- [ ] Test : crash entre écriture du document et requête `completed`, aucun effet.
- [ ] Porter réconciliation et purge, y compris départements gated sur les références.
- [ ] Vérifier : toute la parité API verte contre Rust.

### T10 — Admin : session et écrans de gestion (S)

**Files :** `admin/session.rs`, `admin/csrf.rs`, `admin/departments.rs`,
`app_clients.rs`, `upload_requests.rs`, `status.rs`, `templates/`.

- [ ] Session signée, argon2, CSRF, CSP, tests d'abord.
- [ ] Layouts et écrans en réutilisant CSS et JS sans les modifier.
- [ ] Porter `SystemHealth` et `DiskSpace`.
- [ ] Vérifier : parité admin de ces écrans verte contre Rust.

### T11 — Admin : documents (G)

**Files :** `admin/documents.rs`, `admin/trash.rs`, `templates/documents/`.
**Consumes :** session et layouts de T10, `DocumentCreator`.

- [x] Liste, upload, édition, aperçu avec CSP, corbeille, actions en lot.
- [x] ZIP en streaming, test de mémoire stable sur gros volume, Review Focus 1.
- [x] Vérifier : toute la parité admin et Playwright vertes contre Rust.
- Playwright contre le serveur Rust : `rust/e2e/README.md` (stack compose,
  `ged serve`, `GED_BASE_URL`, identifiants admin). Le `web` compose reste Rails.

### T12 — OpenTelemetry (G)

**Files :** `telemetry.rs`.

- [ ] Test : SDK inactif sans `OTEL_EXPORTER_OTLP_ENDPOINT`.
- [ ] Traces et les trois métriques `ged.*`, attributs fermés.
- [ ] Vérifier : dashboard `GED overview` alimenté avec `GED_OTEL=1`.

### T13 — Fuzz différentiel (S)

**Files :** `rust/parity/src/bin/diff_fuzz.rs`, `docs/migration/parity-gaps.md`.

- [x] Générer des requêtes API, normaliser identifiants et dates, comparer Rails/Rust.
- [x] Corriger le Rust ou documenter chaque écart.
- [x] Vérifier : 100 000 requêtes sans écart non documenté.
      2026-11-11, workers arrêtés, deux graines, `undocumented=0`.
      Graine 13 : `elapsed=233s requests=100000 documented=1769`.
      Graine 29 : `elapsed=236s requests=100000 documented=1604`.
      Commande, distributions et premières lignes documentées :
      `docs/migration/parity-gaps.md`, section T13 « Campagne ».

### T14 — Docker, stack et CI (S)

**Files :** `Dockerfile`, `docker-compose.yml`, `bin/worktree-stack`, `bin/ci`,
`.github/workflows/ci.yml`, tests de garde.

- [x] Image Rust unique pour `web` et `worker`, volume et ports inchangés.
- [x] `db`, `db-port`, `up`, `scale`, `clean` pilotent le Rust.
- [x] Porter les gardes C4, API docs et couverture des queues.
- [ ] Vérifier : `bin/worktree-stack up`, `scale 3`, parité complète contre la stack.
      Mesure du 2026-11-11, avant le correctif de `bin/ci` : `GED_STACK=rails bin/worktree-stack up` sert `/up`
      en 200 avec `X-Runtime`. `bin/worktree-stack up` puis `scale 3` sont verts
      (`/up` 200, trois conteneurs `ged worker`). Ce jour-là `bin/ci` lançait
      `cargo test` sur tout le workspace, donc la parité avec `GED_BACKEND=rails`
      par défaut. Contre la pile Rust, 10 tests `security_purge` échouaient :
      `run_job` exécute `bin/rails runner` dans `web`, absent de l'image Rust.
      La passe manuelle `GED_BACKEND=rust cargo test -p parity --no-fail-fast`
      sortait 101 avec 26 échecs, tous le trou T11 (`admin/mod.rs` :
      `documents_stub` à la place de la liste, de l'aperçu et de la corbeille) :
      `admin_documents` 14, `admin_params` 6 (l'index documents ; les 2 tests
      upload_requests passent), `admin_trash` 5 (404), et
      `api_documents::api_delete_soft_deletes_the_document_and_keeps_it_for_the_retention_window`
      (`PATCH /admin/deleted_documents/:id/restore` 404 ; Rails, le même test,
      répond 302). Aucun autre test rouge. Les deux images sont construites par
      le job `rust` et par `bin/ci` quand Docker répond.
      Correctif : `Rust: tests` est `cargo test -p ged`. L'étape
      `Parity: rust` ou `Parity: rails` lance
      `GED_BACKEND=<pile> cargo test -p parity` seulement si `GET /up` de la
      pile du worktree répond. La pile est `bin/worktree-stack active`
      (`GED_STACK` s'il est posé, sinon le nom mémorisé par `up`, sinon
      `rust`). Sans réponse, `bin/ci` affiche `Parity: skipped` et ne compte
      pas ce saut comme une parité verte. GitHub CI reste sur `cargo test -p ged`
      (pas de pile compose sur le runner).

### T15 — Validation perf et chaos (S)

**Files :** `docs/perf/2026-11-01-rust.md`.

- [x] Lancer `bin/perf-upload` sur les 3 profils à 1 et 3 workers, sous `caffeinate -i`,
  puis `--chaos` web et worker, puis `bin/monkey`.
- [x] Écrire uniquement des résultats réels après les runs.
- [x] Vérifier : zéro perte, zéro doublon, aucun 5xx. Toute régression est rapportée.
      Rapport `docs/perf/2026-11-01-rust.md` (2026-11-11). Le harness du chaos web
      imprime `zero-loss: FAILED` parce qu'un 202 n'a pas été lu par le client ;
      le SQL compte 552 documents et 552 noms distincts, sans doublon. Un 202 non lu
      par le client est l'explication probable de l'écart, non prouvée par upload
      (le harness ne journalise pas les index acquittés) : zéro perte n'est établi
      que sur les totaux.

### T16 — Suppression de Rails et documentation (S)

**Files :** suppression de `app/`, `config/`, `db/`, `test/`, `Gemfile*`, etc. ;
réécriture de `AGENTS.md`, `docs/architecture/c4.md`.

- [ ] Conserver `oauth-provider/`, `bin/perf-upload`, `bin/monkey`, `docs/`.
- [ ] Retirer l'adaptateur Rails de `rust/parity`, garder le fichier des écarts.
- [ ] ADR de clôture dans codebase memory, réindexation.
- [ ] Vérifier : `bin/ci` vert sans Rails, `bin/worktree-stack up` prêt pour ta
  validation. Le merge vers `main` attend ta confirmation explicite.

### Vague 10 — Revue finale de la branche (Fable 5.2, demande du propriétaire)

Un worker Fable 5.2 relit, en lecture seule, toute la branche d'intégration contre `main`,
après l'intégration de T16 et avant la stack de validation du propriétaire.

- [ ] Vérifier au lancement que l'identifiant du modèle Fable 5.2 est accepté. S'il ne
  l'est pas, faire un rapport au propriétaire sans substituer un autre modèle.
- [ ] Périmètre : la spec entière contre le code, la sécurité d'abord (auth JWT et session,
  SSRF, CSRF, en-têtes, limites, chemins de fichiers), puis l'exactitude de la file de
  jobs (exactement une fois pour les documents), la parité des erreurs et les écarts admis
  de `parity-gaps.md`, la doc (`AGENTS.md`, `c4.md`, `openapi.yaml`).
- [ ] Livrable : rapport avec verdict APPROVE ou CHANGES et constats `fichier:ligne`
  classés par gravité. Les correctifs éventuels repartent vers l'auteur de la tâche
  concernée, puis une nouvelle vérification complète par l'orchestrateur.
- [ ] Ensuite seulement : `bin/worktree-stack up` pour la validation du propriétaire.

## Reports de la relecture de la vague 3

- **T4 :** ajouter l'adaptateur `GED_BACKEND=rust`, sortir les détails Rails hors de
  l'adaptateur (`security_purge.rs`, `assert_rails_*`, message de `admin_documents.rs`).
  Noter dans `parity-gaps.md` l'état JWKS admis par `admin_status.rs`. Renforcer les
  trois tests de `admin_departments.rs` qui ne vérifient pas le statut.
- **T6/T9 :** test unitaire de suppression physique du fichier à la purge.
- **T7 :** test d'upload exactement à la limite, 413 à corps vide reproduit et documenté
  dans `docs/api/openapi.yaml`.
- **T9 :** cas SSRF `999.1.1.1` et `[::1]`.

## Contrats de la vague 6 (décidés par l'orchestrateur)

- Jobs : `JobPayload::new(kind, args)` + `queue::enqueue`. Kinds et args figés :
  `StoreDocumentJob` `{"upload_request_id": uuid}` sur `uploads` ;
  `DeliverCallbackJob` `{"upload_request_id": uuid}`, `ReconcileUploadRequestsJob` `{}`,
  `PurgeDeletedDocumentsJob` `{}` sur `default`. T7 enqueue, T9 implémente et enregistre.
- `rust/ged/src/policy/callback_url.rs` appartient à T7 : contrôle à la requête et
  `decide(url, resolve: true) -> Result<Decision, Refusal>` avec l'adresse résolue à
  épingler. T9 le consomme sans le modifier.
- `rust/ged/src/domain/upload_request.rs` appartient à T7.
- Reports de la relecture T6 : garde `purge_at` à la purge, orphelins `.part` et échec
  de suppression (T9) ; restore conditionnel (T7) ; CSP d'aperçu (T11). Dashboard
  `GED_OTEL=1` sur stack Docker complète : T14.
- `rust/ged/src/admin/documents_stub.rs` (T10) était provisoire. T11 l'a
  supprimé et l'a remplacé par `documents.rs` et `trash.rs`.

## Répartition des modèles révisée (2026-11-03, demande du propriétaire)

Ponctuellement, pour ménager le quota Claude de ce chantier : T11, T13, T14, T15 et T16
passent sur Grok (effort high, config du propriétaire). Sonnet 5.5 medium reste sur les
relectures, les intégrations et les correctifs des tâches qu'il a écrites (T7, T9, T10).
- Report T7 C2 : balayage des fichiers orphelins de staging après crash (Review Focus 2),
  à faire par le propriétaire de T9 après l'intégration de la vague 6.

## Reports de la vague 6 (intégrée en f296ea9)

- T11 (fait) : `documents.rs` et `trash.rs` remplacent `admin/documents_stub.rs`.
  Les 26 tests admin (`admin_documents`, `admin_params`, `admin_trash`) passent,
  la parité complète est verte sur les deux backends (286), Playwright aussi
  (22) contre le serveur Rust. CSP d'aperçu et statiques sans en-têtes de
  contrôleur (`cache-control: public, max-age` d'un an). L'écart « id non UUID
  → Rails 500 » était faux : Rails caste en nil et répond 404 (page statique),
  ou, en lot, « 0 documents moved to trash. » / ZIP vide. Voir `parity-gaps.md`.
- T14 : le `ged worker` lancé par l'adaptateur Rust de la parité vise une base supprimée
  entre deux tests (logs « database ged_rust_parity does not exist ») ; synchroniser son
  cycle de vie avec `reset()`.
- C2 de T7 : balayage des fichiers orphelins de staging après crash.

## Reports de la vague 7

- `rust/ged/src/storage/files.rs`, `copy_limited` : si le futur est annulé pendant
  `tokio::fs::rename` (exécuté via `spawn_blocking`), le `Drop` du `PartGuard` peut passer
  avant le renommage effectif et laisser `dest` orphelin. À corriger plus tard par le
  propriétaire du stockage.
- La parité était lancée contre la stack Rust démarrée à la main (`bin/worktree-stack up`),
  pas dans `bin/ci`. T14 la fait suivre la pile active : voir le correctif de `bin/ci` dans T14.

## Reports de la vague 8

Constats mineurs de la relecture de T13 (`review-t13.md`), non traités ici. Les
points H1, M1, M2 et M3 de cette relecture sont suivis séparément ; ceux-ci
restent ouverts.

- **M4 — familles absentes du générateur `diff_fuzz` :** mauvais scope
  (`GED_OIDC_TOKEN_BINDING=scope`) ; client sans grant et second client ;
  callbacks et transitions de l'upload asynchrone ; `bulk_destroy` au-delà de
  500 ids, doc déjà supprimé ou hors périmètre write mélangé ; corps trop gros
  (413), `PART_LIMIT`, `FORM_PARTS_LIMIT`, multipart sans boundary, tronqué ou en
  LF seul ; corbeille (départements, docs, ancêtre supprimé, restauration) ;
  comparaison de l'état DB entre les deux piles ; noms de fichiers hostiles
  (`\r`, non UTF-8) et HEAD hors `/ping`.
- **M5 — tri des départements :** `ORDER BY name` sans tie-break `id`
  (`rust/ged/src/api/departments.rs`), fidèle à Rails mais instable sur les
  ex æquo (unicité par `parent_id`). Ajouter un test de non-régression et
  documenter l'écart d'ordre.
- **M6 — correctif CR/LF multipart :** le fuzzer ne génère que `\n` ; `\r` n'est
  couvert que par un test unitaire ; généralisation à tout en-tête de toute
  partie non observée ; pas de test HTTP/parité ni de cas première/dernière
  partie cassée ou corps tronqué.
- **L1 :** `BadRequest(m) if m == "bad request"` décide le corps « bare » sur une
  chaîne (`http/error.rs`) ; préférer une variante dédiée.
- **L2 — `bin/worktree-stack` avec `GED_COMPOSE_PROJECT` :** `clean`, `scale` et
  `exec` suivent `active_stack`, donc exiger `GED_STACK` sur chaque commande
  d'un projet latéral d'une autre image ; les ports ne sont pas forcés.
- **L3 :** `send_retry` de `diff_fuzz` rejoue les POST (doublon d'un seul côté,
  faux écart possible, jamais un masquage).
- **L4 — normalisation :** UUID et dates numérotés par ordre d'apparition,
  ETag réduit à faible/forte ; le fuzzer ne prouve ni les valeurs de dates ni
  l'ETag.
- **L5 — `parity-gaps.md` :** ne mentionne pas HEAD (`Content-Length: 0`), le
  content-type par défaut `text/html`, le tri des ex æquo ni le rejet des dates
  sans heure.
- **Rapport perf T15 (relecture T15, LOW) :** sonde `auth/aud_unverified` de
  `bin/monkey` à durcir (token à `aud` étranger ou absent, 401 attendu) ;
  `revoke_temp_grant` a un `rescue StandardError` mort (l'échec aborte) ;
  `PERF_CLIENT_ID` interpolé dans le SQL sans échappement d'apostrophe ;
  `ON CONFLICT … DO UPDATE` ne réinitialise pas `deleted_at` ; la phrase sur
  le p99 plus haut côté Rust omet mix 3w (180 vs 152) et many-small 1w
  (29 vs 26).
