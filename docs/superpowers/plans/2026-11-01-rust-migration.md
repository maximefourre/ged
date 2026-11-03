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
- `cargo fmt --check`, `cargo clippy -- -D warnings` et `cargo test` verts à chaque
  tâche. `bin/ci` Rails reste vert tant que Rails existe.
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

- [ ] Liste, upload, édition, aperçu avec CSP, corbeille, actions en lot.
- [ ] ZIP en streaming, test de mémoire stable sur gros volume, Review Focus 1.
- [ ] Vérifier : toute la parité admin et Playwright vertes contre Rust.

### T12 — OpenTelemetry (G)

**Files :** `telemetry.rs`.

- [ ] Test : SDK inactif sans `OTEL_EXPORTER_OTLP_ENDPOINT`.
- [ ] Traces et les trois métriques `ged.*`, attributs fermés.
- [ ] Vérifier : dashboard `GED overview` alimenté avec `GED_OTEL=1`.

### T13 — Fuzz différentiel (S)

**Files :** `rust/parity/src/bin/diff_fuzz.rs`, `docs/migration/parity-gaps.md`.

- [ ] Générer des requêtes API, normaliser identifiants et dates, comparer Rails/Rust.
- [ ] Corriger le Rust ou documenter chaque écart.
- [ ] Vérifier : 100 000 requêtes sans écart non documenté.

### T14 — Docker, stack et CI (S)

**Files :** `Dockerfile`, `docker-compose.yml`, `bin/worktree-stack`, `bin/ci`,
`.github/workflows/ci.yml`, tests de garde.

- [ ] Image Rust unique pour `web` et `worker`, volume et ports inchangés.
- [ ] `db`, `db-port`, `up`, `scale`, `clean` pilotent le Rust.
- [ ] Porter les gardes C4, API docs et couverture des queues.
- [ ] Vérifier : `bin/worktree-stack up`, `scale 3`, parité complète contre la stack.

### T15 — Validation perf et chaos (S)

**Files :** `docs/perf/2026-11-01-rust.md`.

- [ ] Lancer `bin/perf-upload` sur les 3 profils à 1 et 3 workers, sous `caffeinate -i`,
  puis `--chaos` web et worker, puis `bin/monkey`.
- [ ] Écrire uniquement des résultats réels après les runs.
- [ ] Vérifier : zéro perte, zéro doublon, aucun 5xx. Toute régression est rapportée.

### T16 — Suppression de Rails et documentation (S)

**Files :** suppression de `app/`, `config/`, `db/`, `test/`, `Gemfile*`, etc. ;
réécriture de `AGENTS.md`, `docs/architecture/c4.md`.

- [ ] Conserver `oauth-provider/`, `bin/perf-upload`, `bin/monkey`, `docs/`.
- [ ] Retirer l'adaptateur Rails de `rust/parity`, garder le fichier des écarts.
- [ ] ADR de clôture dans codebase memory, réindexation.
- [ ] Vérifier : `bin/ci` vert sans Rails, `bin/worktree-stack up` prêt pour ta
  validation. Le merge vers `main` attend ta confirmation explicite.

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
- `rust/ged/src/admin/documents_stub.rs` (T10) est provisoire : GET et POST minimaux
  pour la session admin et la page status. T11 le supprime et le remplace dans
  `documents.rs`.

## Répartition des modèles révisée (2026-11-03, demande du propriétaire)

Ponctuellement, pour ménager le quota Claude de ce chantier : T11, T13, T14, T15 et T16
passent sur Grok (effort high, config du propriétaire). Sonnet 5.5 medium reste sur les
relectures, les intégrations et les correctifs des tâches qu'il a écrites (T7, T9, T10).
- Report T7 C2 : balayage des fichiers orphelins de staging après crash (Review Focus 2),
  à faire par le propriétaire de T9 après l'intégration de la vague 6.

## Reports de la vague 6 (intégrée en f296ea9)

- T11 : remplacer `admin/documents_stub.rs`, faire passer les 26 tests Rust restants
  (`admin_documents`, `admin_params`, `admin_trash`, restauration via corbeille), CSP
  d'aperçu, fichiers statiques sans les en-têtes de sécurité contrôleur comme Rails
  (`cache-control: public, max-age` d'un an).
- T14 : le `ged worker` lancé par l'adaptateur Rust de la parité vise une base supprimée
  entre deux tests (logs « database ged_rust_parity does not exist ») ; synchroniser son
  cycle de vie avec `reset()`.
- C2 de T7 : balayage des fichiers orphelins de staging après crash.
