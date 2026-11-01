# Inventaire de parité — tests Ruby → tests Rust

Chaque test Ruby (`test "..." do`) est classé dans une catégorie, avec le fichier cible
qui le remplace. Généré le 2026-11-01 d'après `test/` (T1).

**Total : 404 tests** (comptage réel : `grep -rhE '^\s*test ["'"'"']' test | wc -l` = 404).
Les tests de `oauth-provider/test/` ne sont pas comptés : ce service reste en Ruby.

| Catégorie | Tests | Signification |
|---|---:|---|
| parité | 229 | boîte noire HTTP/SQL dans `rust/parity` (`cargo test -p parity`), validée d'abord contre Rails |
| unitaire Rust | 153 | test unitaire ou d'intégration interne dans `rust/ged` (invisible en HTTP) |
| navigateur | 22 | Playwright dans `rust/e2e`, fermeture explicite du navigateur en `finally` |

Règles de classement :

- **parité** : le comportement est observable par HTTP (corps, statut, en-têtes) ou par les
  tables métier (`departments`, `documents`, `upload_requests`, `app_clients`,
  `client_department_accesses`, `admins`). Jamais de lecture des tables de stockage ni de file.
- **unitaire Rust** : pinning DNS, cache JWKS, crash entre les deux écritures de
  `StoreDocumentJob`, fichiers de staging, tables de file, boot fail-closed, métriques, docs
  introspectives (C4, OpenAPI), logique de modèle sans surface HTTP propre.
- **navigateur** : tout `test/system/admin_ui_test.rb` (JS de l'admin, dropzone,
  multi-sélection, thème) ; les parcours sans JS de ce fichier sont aussi couverts en parité
  par `test/controllers/admin/*`.
- Les callbacks async (POST vers `callback_url`) ne sont pas observables en boîte noire (les
  cibles loopback sont bloquées par la garde SSRF) : la charge utile est vérifiée par le
  polling en parité et par `deliver_callback` en unitaire.

## État d'avancement

- Porté (T1) : `test/security/auth_matrix_test.rb` (14 tests) et `test/controllers/api/v1/authentication_test.rb`
  (5 tests, recoupés) → `rust/parity/tests/auth_matrix.rs` (16 tests : les 14 de la matrice,
  un témoin « jeton re-signé sans modification accepté », et le schéma non-Bearer).
  Vert contre Rails : `GED_BACKEND=rails cargo test -p parity auth`.
- `rust/parity/tests/harness_admin.rs` : tests de fumée de l'utilitaire de session admin
  (login formulaire, CSRF, logout), hors inventaire.
- Porté (T3b) : `test/system/admin_ui_test.rb` (22 tests navigateur) →
  `rust/e2e/tests/admin_ui.spec.ts`. Session réutilisée via `AdminSession::cached`
  (`rust/parity/src/bin/admin_session.rs`) à cause de la limite de 10 connexions / 3 min.
- Porté (T2a, API) : 84 lignes parité issues de `test/controllers/api` (hors
  `authentication_test.rb`, déjà recoupé en T1) et des lignes OpenAPI/`/up`
  associées (176, 177, 179, 186) → `rust/parity/tests/api_*.rs` et
  `rust/parity/tests/health.rs` (`api_up_returns_200`). Vert contre Rails :
  `GED_BACKEND=rails cargo test -p parity api` (84 tests). La charge utile des
  callbacks async est observée par le polling ; le POST de callback reste unitaire.
  Lignes 87 et 90 sont **unitaire Rust** (`rust/ged/src/jobs/store_document.rs` et
  `rust/ged/src/jobs/deliver_callback.rs`) : elles suppriment `storage/staging`,
  qui n'est pas une table métier.
- Les tests de polling qui figent l'état `pending` appellent
  `POST`/`DELETE /admin/upload_queue_pause`. Le port Rust doit servir cette
  route admin dès que ces tests tournent contre Rust (T3a/T8).
- Ligne 178 (couverture des chemins OpenAPI) reste unitaire Rust.
- Porté (sécurité) : `authz_idor_test.rb` (12), les 19 lignes parité de `input_fuzz_test.rb`,
  les 4 encodages numériques de `ssrf_test.rb` (422) et `purge_deleted_documents_job_test.rb` (9)
  → `rust/parity/tests/security_{authz,fuzz,ssrf,purge}.rs`. 56 tests Rust au total, dont la
  matrice Rack (`tags[type]=x`, `limit[]=1` → 200, doublons, vide ou absent), un 202 vers un
  hostname public, et le parcours HTTP soft-delete puis `PurgeDeletedDocumentsJob`.
  Vert : `GED_BACKEND=rails cargo test -p parity security` (56 passed).
  La ligne 92 (corps du POST de callback) est reclassée unitaire
  (`rust/ged/src/jobs/deliver_callback.rs`) : la cible loopback est bloquée par la garde SSRF.
- Tout le reste : à porter dans T3a (admin HTTP) et T4+ (unitaire Rust).

## Répartition par fichier Ruby

| Fichier Ruby | Tests | parité | unitaire Rust | navigateur |
|---|---:|---:|---:|---:|
| `test/controllers/admin/admin_ui_test.rb` | 26 | 26 | 0 | 0 |
| `test/controllers/admin/app_clients_controller_test.rb` | 7 | 7 | 0 | 0 |
| `test/controllers/admin/authentication_test.rb` | 3 | 3 | 0 | 0 |
| `test/controllers/admin/deleted_documents_controller_test.rb` | 5 | 5 | 0 | 0 |
| `test/controllers/admin/departments_controller_test.rb` | 15 | 15 | 0 | 0 |
| `test/controllers/admin/documents_controller_test.rb` | 4 | 4 | 0 | 0 |
| `test/controllers/admin/login_rate_limit_test.rb` | 2 | 2 | 0 | 0 |
| `test/controllers/admin/pagination_test.rb` | 4 | 4 | 0 | 0 |
| `test/controllers/admin/param_guards_test.rb` | 4 | 4 | 0 | 0 |
| `test/controllers/admin/status_controller_test.rb` | 5 | 4 | 1 | 0 |
| `test/controllers/admin/upload_queue_pause_test.rb` | 3 | 3 | 0 | 0 |
| `test/controllers/admin/upload_requests_controller_test.rb` | 4 | 4 | 0 | 0 |
| `test/controllers/api/v1/async_upload_test.rb` | 12 | 10 | 2 | 0 |
| `test/controllers/api/v1/authentication_test.rb` | 5 | 5 | 0 | 0 |
| `test/controllers/api/v1/departments_test.rb` | 12 | 12 | 0 | 0 |
| `test/controllers/api/v1/document_tags_test.rb` | 7 | 7 | 0 | 0 |
| `test/controllers/api/v1/documents_bulk_destroy_test.rb` | 7 | 7 | 0 | 0 |
| `test/controllers/api/v1/documents_test.rb` | 28 | 28 | 0 | 0 |
| `test/controllers/api/v1/read_write_permissions_test.rb` | 14 | 14 | 0 | 0 |
| `test/controllers/api/v1/upload_requests_test.rb` | 2 | 2 | 0 | 0 |
| `test/initializers/oidc_config_test.rb` | 6 | 0 | 6 | 0 |
| `test/integration/api_docs_test.rb` | 4 | 3 | 1 | 0 |
| `test/integration/c4_diagrams_test.rb` | 6 | 0 | 6 | 0 |
| `test/integration/health_check_test.rb` | 1 | 1 | 0 | 0 |
| `test/jobs/deliver_callback_job_test.rb` | 16 | 0 | 16 | 0 |
| `test/jobs/purge_deleted_documents_job_test.rb` | 9 | 9 | 0 | 0 |
| `test/jobs/queue_coverage_test.rb` | 1 | 0 | 1 | 0 |
| `test/jobs/reconcile_upload_requests_job_test.rb` | 10 | 1 | 9 | 0 |
| `test/jobs/store_document_job_test.rb` | 8 | 0 | 8 | 0 |
| `test/lib/telemetry_test.rb` | 3 | 0 | 3 | 0 |
| `test/models/admin_test.rb` | 2 | 0 | 2 | 0 |
| `test/models/app_client_test.rb` | 7 | 0 | 7 | 0 |
| `test/models/client_department_access_test.rb` | 2 | 0 | 2 | 0 |
| `test/models/department_test.rb` | 15 | 0 | 15 | 0 |
| `test/models/document_test.rb` | 11 | 0 | 11 | 0 |
| `test/models/tags_map_validation_test.rb` | 8 | 0 | 8 | 0 |
| `test/models/upload_request_test.rb` | 4 | 0 | 4 | 0 |
| `test/security/auth_matrix_test.rb` | 14 | 14 | 0 | 0 |
| `test/security/authz_idor_test.rb` | 12 | 12 | 0 | 0 |
| `test/security/input_fuzz_test.rb` | 21 | 19 | 2 | 0 |
| `test/security/ssrf_test.rb` | 8 | 4 | 4 | 0 |
| `test/seeds_test.rb` | 1 | 0 | 1 | 0 |
| `test/services/auth/jwks_cache_test.rb` | 6 | 0 | 6 | 0 |
| `test/services/auth/token_validator_test.rb` | 14 | 0 | 14 | 0 |
| `test/services/callback_url_policy_test.rb` | 9 | 0 | 9 | 0 |
| `test/services/disk_space_test.rb` | 2 | 0 | 2 | 0 |
| `test/services/document_creator_test.rb` | 3 | 0 | 3 | 0 |
| `test/services/system_health_test.rb` | 10 | 0 | 10 | 0 |
| `test/system/admin_ui_test.rb` | 22 | 0 | 0 | 22 |
| **Total** | **404** | **229** | **153** | **22** |

## Inventaire détaillé

| # | Fichier Ruby:ligne | Test | Catégorie | Cible | Note |
|---:|---|---|---|---|---|
| 1 | `test/controllers/admin/admin_ui_test.rb:13` | creates a top-level department | parité | `rust/parity/tests/admin_departments.rs` |  |
| 2 | `test/controllers/admin/admin_ui_test.rb:19` | creates a subfolder | parité | `rust/parity/tests/admin_departments.rs` |  |
| 3 | `test/controllers/admin/admin_ui_test.rb:24` | renames a department | parité | `rust/parity/tests/admin_departments.rs` |  |
| 4 | `test/controllers/admin/admin_ui_test.rb:34` | rejects renaming a department to a blank name | parité | `rust/parity/tests/admin_departments.rs` |  |
| 5 | `test/controllers/admin/admin_ui_test.rb:40` | manages a document's tags through the key=value textarea | parité | `rust/parity/tests/admin_documents.rs` |  |
| 6 | `test/controllers/admin/admin_ui_test.rb:65` | the tags edit page shows readonly document metadata | parité | `rust/parity/tests/admin_documents.rs` |  |
| 7 | `test/controllers/admin/admin_ui_test.rb:78` | filters documents by tag key | parité | `rust/parity/tests/admin_documents.rs` |  |
| 8 | `test/controllers/admin/admin_ui_test.rb:94` | deletes a department granted to an app client | parité | `rust/parity/tests/admin_departments.rs` |  |
| 9 | `test/controllers/admin/admin_ui_test.rb:102` | deleting a department with upload requests redirects to the resolution page (no 500) | parité | `rust/parity/tests/admin_departments.rs` |  |
| 10 | `test/controllers/admin/admin_ui_test.rb:111` | deleting a department whose only documents are soft-deleted redirects to the resolution page (no 500) | parité | `rust/parity/tests/admin_departments.rs` |  |
| 11 | `test/controllers/admin/admin_ui_test.rb:124` | registers an app client with full read/write access from the grid | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 12 | `test/controllers/admin/admin_ui_test.rb:135` | registers a read-only app client | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 13 | `test/controllers/admin/admin_ui_test.rb:146` | registers a write-only app client | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 14 | `test/controllers/admin/admin_ui_test.rb:157` | unchecking both boxes removes the department access row | parité | `rust/parity/tests/admin_departments.rs` |  |
| 15 | `test/controllers/admin/admin_ui_test.rb:169` | updating access flags edits the existing row in place | parité | `rust/parity/tests/admin_documents.rs` |  |
| 16 | `test/controllers/admin/admin_ui_test.rb:182` | the app client form shows the read/write grid | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 17 | `test/controllers/admin/admin_ui_test.rb:190` | the legacy department_ids param no longer grants access | parité | `rust/parity/tests/admin_departments.rs` |  |
| 18 | `test/controllers/admin/admin_ui_test.rb:199` | uploads a document, then soft-deletes it to the trash | parité | `rust/parity/tests/admin_documents.rs` |  |
| 19 | `test/controllers/admin/admin_ui_test.rb:220` | uploads a document with tags | parité | `rust/parity/tests/admin_documents.rs` |  |
| 20 | `test/controllers/admin/admin_ui_test.rb:230` | uploading a non-file file param shows an alert instead of a 500 | parité | `rust/parity/tests/admin_documents.rs` |  |
| 21 | `test/controllers/admin/admin_ui_test.rb:239` | lists upload requests with status and error | parité | `rust/parity/tests/admin_upload_requests.rs` |  |
| 22 | `test/controllers/admin/admin_ui_test.rb:251` | downloads a document | parité | `rust/parity/tests/admin_documents.rs` |  |
| 23 | `test/controllers/admin/admin_ui_test.rb:260` | bulk_destroy soft-deletes the selected documents | parité | `rust/parity/tests/admin_documents.rs` |  |
| 24 | `test/controllers/admin/admin_ui_test.rb:272` | bulk_destroy with an empty selection redirects with an alert and deletes nothing | parité | `rust/parity/tests/admin_documents.rs` |  |
| 25 | `test/controllers/admin/admin_ui_test.rb:282` | bulk_download streams a ZIP with id-prefixed entry names and correct bytes | parité | `rust/parity/tests/admin_documents.rs` |  |
| 26 | `test/controllers/admin/admin_ui_test.rb:296` | bulk_download dedupes same-filename documents by id prefix | parité | `rust/parity/tests/admin_documents.rs` |  |
| 27 | `test/controllers/admin/app_clients_controller_test.rb:17` | requires an admin session | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 28 | `test/controllers/admin/app_clients_controller_test.rb:22` | index lists clients with their granted department name | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 29 | `test/controllers/admin/app_clients_controller_test.rb:32` | index renders for a client whose granted department is trashed | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 30 | `test/controllers/admin/app_clients_controller_test.rb:46` | edit form surfaces a granted trashed department marked trashed | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 31 | `test/controllers/admin/app_clients_controller_test.rb:59` | admin can remove a grant to a trashed department | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 32 | `test/controllers/admin/app_clients_controller_test.rb:75` | re-saving keeps an untouched grant to a trashed department | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 33 | `test/controllers/admin/app_clients_controller_test.rb:97` | index shows an empty state when there are no clients | parité | `rust/parity/tests/admin_app_clients.rs` |  |
| 34 | `test/controllers/admin/authentication_test.rb:6` | unauthenticated access to admin area redirects to login | parité | `rust/parity/tests/admin_auth.rs` |  |
| 35 | `test/controllers/admin/authentication_test.rb:11` | authenticated admin reaches admin area | parité | `rust/parity/tests/admin_auth.rs` |  |
| 36 | `test/controllers/admin/authentication_test.rb:20` | failed login re-renders the form with the devise alert visible | parité | `rust/parity/tests/admin_auth.rs` |  |
| 37 | `test/controllers/admin/deleted_documents_controller_test.rb:22` | index lists soft-deleted documents and not live ones | parité | `rust/parity/tests/admin_trash.rs` |  |
| 38 | `test/controllers/admin/deleted_documents_controller_test.rb:33` | restore brings a soft-deleted document back into the normal index | parité | `rust/parity/tests/admin_trash.rs` |  |
| 39 | `test/controllers/admin/deleted_documents_controller_test.rb:45` | requires an admin session | parité | `rust/parity/tests/admin_trash.rs` |  |
| 40 | `test/controllers/admin/deleted_documents_controller_test.rb:50` | single-doc restore un-greys its department but leaves siblings trashed | parité | `rust/parity/tests/admin_trash.rs` |  |
| 41 | `test/controllers/admin/deleted_documents_controller_test.rb:78` | trash page renders for doc whose department is trashed | parité | `rust/parity/tests/admin_trash.rs` |  |
| 42 | `test/controllers/admin/departments_controller_test.rb:11` | destroy on a dept holding LIVE docs redirects to confirm_deletion | parité | `rust/parity/tests/admin_departments.rs` |  |
| 43 | `test/controllers/admin/departments_controller_test.rb:25` | destroy on a dept with ALL docs TRASHED redirects to confirm_deletion | parité | `rust/parity/tests/admin_departments.rs` |  |
| 44 | `test/controllers/admin/departments_controller_test.rb:40` | destroy on a dept with pending upload_requests redirects to confirm_deletion | parité | `rust/parity/tests/admin_departments.rs` |  |
| 45 | `test/controllers/admin/departments_controller_test.rb:55` | destroy on a dept WITH children alerts and doesn't delete | parité | `rust/parity/tests/admin_departments.rs` |  |
| 46 | `test/controllers/admin/departments_controller_test.rb:67` | destroy on a dept with trashed child alerts and doesn't delete | parité | `rust/parity/tests/admin_departments.rs` |  |
| 47 | `test/controllers/admin/departments_controller_test.rb:80` | destroy on an empty dept hard-deletes and shows notice | parité | `rust/parity/tests/admin_departments.rs` |  |
| 48 | `test/controllers/admin/departments_controller_test.rb:90` | move_documents reassigns BOTH live and trashed docs AND upload_requests | parité | `rust/parity/tests/admin_departments.rs` |  |
| 49 | `test/controllers/admin/departments_controller_test.rb:139` | move_documents rejects target == source | parité | `rust/parity/tests/admin_departments.rs` |  |
| 50 | `test/controllers/admin/departments_controller_test.rb:150` | move_documents with a trashed target returns 404 | parité | `rust/parity/tests/admin_departments.rs` |  |
| 51 | `test/controllers/admin/departments_controller_test.rb:161` | trash then restore round-trips | parité | `rust/parity/tests/admin_departments.rs` |  |
| 52 | `test/controllers/admin/departments_controller_test.rb:191` | index renders Documents count column | parité | `rust/parity/tests/admin_departments.rs` |  |
| 53 | `test/controllers/admin/departments_controller_test.rb:206` | index shows greyed row and Restore button for trashed dept | parité | `rust/parity/tests/admin_departments.rs` |  |
| 54 | `test/controllers/admin/departments_controller_test.rb:217` | index shows an empty state when there are no departments | parité | `rust/parity/tests/admin_departments.rs` |  |
| 55 | `test/controllers/admin/departments_controller_test.rb:223` | index lists departments depth-first with tree guides | parité | `rust/parity/tests/admin_departments.rs` |  |
| 56 | `test/controllers/admin/departments_controller_test.rb:235` | index renders a tree-toggle button for a parent department but not a leaf | parité | `rust/parity/tests/admin_departments.rs` |  |
| 57 | `test/controllers/admin/documents_controller_test.rb:21` | preview streams a PDF inline with a viewer-compatible CSP, signed in | parité | `rust/parity/tests/admin_documents.rs` |  |
| 58 | `test/controllers/admin/documents_controller_test.rb:38` | preview hardens non-PDF types with a scriptless sandbox CSP | parité | `rust/parity/tests/admin_documents.rs` |  |
| 59 | `test/controllers/admin/documents_controller_test.rb:58` | preview redirects to login when signed out | parité | `rust/parity/tests/admin_documents.rs` |  |
| 60 | `test/controllers/admin/documents_controller_test.rb:65` | download still streams as an attachment | parité | `rust/parity/tests/admin_documents.rs` |  |
| 61 | `test/controllers/admin/login_rate_limit_test.rb:9` | repeated login attempts are rate limited | parité | `rust/parity/tests/admin_auth.rs` | budget 10 tentatives/3 min par process : une seule session partagée par la suite |
| 62 | `test/controllers/admin/login_rate_limit_test.rb:24` | a few failed attempts do not block a subsequent valid login | parité | `rust/parity/tests/admin_auth.rs` | budget 10 tentatives/3 min par process : une seule session partagée par la suite |
| 63 | `test/controllers/admin/pagination_test.rb:11` | documents index shows 50 per page with navigation links | parité | `rust/parity/tests/admin_params.rs` |  |
| 64 | `test/controllers/admin/pagination_test.rb:27` | upload requests index shows 50 per page and keeps the status filter | parité | `rust/parity/tests/admin_params.rs` |  |
| 65 | `test/controllers/admin/pagination_test.rb:40` | non-numeric or non-scalar page params fall back to the first page | parité | `rust/parity/tests/admin_params.rs` |  |
| 66 | `test/controllers/admin/pagination_test.rb:49` | a page far beyond bigint offset range is clamped, not a 500 | parité | `rust/parity/tests/admin_params.rs` |  |
| 67 | `test/controllers/admin/param_guards_test.rb:16` | documents index ignores an array filename param instead of 500ing | parité | `rust/parity/tests/admin_params.rs` |  |
| 68 | `test/controllers/admin/param_guards_test.rb:21` | documents index ignores a hash department_id param instead of 500ing | parité | `rust/parity/tests/admin_params.rs` |  |
| 69 | `test/controllers/admin/param_guards_test.rb:26` | documents index ignores an array tag param instead of 500ing | parité | `rust/parity/tests/admin_params.rs` |  |
| 70 | `test/controllers/admin/param_guards_test.rb:31` | upload requests index ignores a hash status param instead of 500ing | parité | `rust/parity/tests/admin_params.rs` |  |
| 71 | `test/controllers/admin/status_controller_test.rb:12` | requires an admin session | parité | `rust/parity/tests/admin_status.rs` |  |
| 72 | `test/controllers/admin/status_controller_test.rb:18` | renders the three sections with live counts | parité | `rust/parity/tests/admin_status.rs` |  |
| 73 | `test/controllers/admin/status_controller_test.rb:42` | reflects the uploads queue pause state | parité | `rust/parity/tests/admin_status.rs` |  |
| 74 | `test/controllers/admin/status_controller_test.rb:51` | top departments table lists stored volume and document count | parité | `rust/parity/tests/admin_status.rs` |  |
| 75 | `test/controllers/admin/status_controller_test.rb:67` | lists recent failed jobs | unitaire Rust | `rust/ged/src/admin/status.rs` | jobs échoués : table de file, hors fixtures |
| 76 | `test/controllers/admin/upload_queue_pause_test.rb:12` | pauses and resumes the uploads queue | parité | `rust/parity/tests/admin_upload_queue.rs` |  |
| 77 | `test/controllers/admin/upload_queue_pause_test.rb:23` | the banner reflects queue state and request counts | parité | `rust/parity/tests/admin_upload_queue.rs` |  |
| 78 | `test/controllers/admin/upload_queue_pause_test.rb:39` | requires an admin session | parité | `rust/parity/tests/admin_upload_queue.rs` |  |
| 79 | `test/controllers/admin/upload_requests_controller_test.rb:17` | requires an admin session | parité | `rust/parity/tests/admin_upload_requests.rs` |  |
| 80 | `test/controllers/admin/upload_requests_controller_test.rb:22` | index lists requests with their department name | parité | `rust/parity/tests/admin_upload_requests.rs` |  |
| 81 | `test/controllers/admin/upload_requests_controller_test.rb:32` | index renders for a request whose department is trashed | parité | `rust/parity/tests/admin_upload_requests.rs` |  |
| 82 | `test/controllers/admin/upload_requests_controller_test.rb:45` | index shows an empty state when there are no upload requests | parité | `rust/parity/tests/admin_upload_requests.rs` |  |
| 83 | `test/controllers/api/v1/async_upload_test.rb:19` | upload with a callback_url targeting an internal address returns 422 | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 84 | `test/controllers/api/v1/async_upload_test.rb:28` | upload with callback_url returns 202 and processes in background | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) ; octets vérifiés par le download ; suppression du fichier de staging : unitaire (store_document) |
| 85 | `test/controllers/api/v1/async_upload_test.rb:58` | rejects callback_url values that are not http(s) URLs with 422 | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 86 | `test/controllers/api/v1/async_upload_test.rb:71` | upload request status can be polled through pending and completed | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 87 | `test/controllers/api/v1/async_upload_test.rb:92` | an async failure is surfaced through the polling endpoint | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | non porté en parité : le test Ruby supprime `storage/staging` (hors tables métier) ; statut `failed` et `document_id` nul relèvent du job de stockage |
| 88 | `test/controllers/api/v1/async_upload_test.rb:111` | async upload applies tags and returns them in the callback and polling | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 89 | `test/controllers/api/v1/async_upload_test.rb:138` | async upload with invalid tags is rejected with 422 before the 202 | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 90 | `test/controllers/api/v1/async_upload_test.rb:150` | a failed async upload reports null tags in the callback | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` | non porté en parité : échec provoqué en supprimant le fichier de staging ; tags null du POST de callback |
| 91 | `test/controllers/api/v1/async_upload_test.rb:172` | async metadata is null while pending and populated once completed | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 92 | `test/controllers/api/v1/async_upload_test.rb:192` | the callback payload carries the document metadata | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` | POST du callback non observable en boîte noire (SSRF bloque le loopback) ; le polling reste en parité |
| 93 | `test/controllers/api/v1/async_upload_test.rb:206` | polling an upload request of a forbidden department returns 403 | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 94 | `test/controllers/api/v1/async_upload_test.rb:215` | async upload into a forbidden department returns 403 and enqueues nothing | parité | `rust/parity/tests/api_async_upload.rs` | assertions de callback couvertes en unitaire (deliver_callback) |
| 95 | `test/controllers/api/v1/authentication_test.rb:9` | no Authorization header returns 401 | parité | `rust/parity/tests/auth_matrix.rs` | recoupe la matrice auth |
| 96 | `test/controllers/api/v1/authentication_test.rb:14` | garbage token returns 401 | parité | `rust/parity/tests/auth_matrix.rs` | recoupe la matrice auth |
| 97 | `test/controllers/api/v1/authentication_test.rb:19` | expired token returns 401 | parité | `rust/parity/tests/auth_matrix.rs` | recoupe la matrice auth |
| 98 | `test/controllers/api/v1/authentication_test.rb:25` | valid token for an unregistered client returns 403 | parité | `rust/parity/tests/auth_matrix.rs` | recoupe la matrice auth |
| 99 | `test/controllers/api/v1/authentication_test.rb:30` | valid token for a registered client returns 200 | parité | `rust/parity/tests/auth_matrix.rs` | recoupe la matrice auth |
| 100 | `test/controllers/api/v1/departments_test.rb:13` | list returns only accessible departments (subtree included) | parité | `rust/parity/tests/api_departments.rs` |  |
| 101 | `test/controllers/api/v1/departments_test.rb:20` | list omits trashed departments | parité | `rust/parity/tests/api_departments.rs` |  |
| 102 | `test/controllers/api/v1/departments_test.rb:29` | create a subfolder under an accessible department | parité | `rust/parity/tests/api_departments.rs` |  |
| 103 | `test/controllers/api/v1/departments_test.rb:38` | create under a forbidden parent returns 403 | parité | `rust/parity/tests/api_departments.rs` |  |
| 104 | `test/controllers/api/v1/departments_test.rb:44` | create without parent_id returns 400 (top-level is admin-only) | parité | `rust/parity/tests/api_departments.rs` |  |
| 105 | `test/controllers/api/v1/departments_test.rb:50` | duplicate sibling name returns 422 | parité | `rust/parity/tests/api_departments.rs` |  |
| 106 | `test/controllers/api/v1/departments_test.rb:56` | non-string name or parent_id returns 400 instead of a garbage row | parité | `rust/parity/tests/api_departments.rs` |  |
| 107 | `test/controllers/api/v1/departments_test.rb:68` | create with an integer parent_id in a JSON body returns 201 | parité | `rust/parity/tests/api_departments.rs` |  |
| 108 | `test/controllers/api/v1/departments_test.rb:77` | create with a null parent_id in a JSON body returns 400 with an honest message | parité | `rust/parity/tests/api_departments.rs` |  |
| 109 | `test/controllers/api/v1/departments_test.rb:85` | create with a non-scalar (hash) parent_id in a JSON body returns 400 | parité | `rust/parity/tests/api_departments.rs` |  |
| 110 | `test/controllers/api/v1/departments_test.rb:91` | create with a float parent_id in a JSON body returns 400 | parité | `rust/parity/tests/api_departments.rs` |  |
| 111 | `test/controllers/api/v1/departments_test.rb:97` | unique-index race on create returns 422 like the validation path | parité | `rust/parity/tests/api_departments.rs` | réécrit en créations concurrentes réelles (Review Focus 5) |
| 112 | `test/controllers/api/v1/document_tags_test.rb:22` | PUT replaces the whole tag map | parité | `rust/parity/tests/api_tags.rs` |  |
| 113 | `test/controllers/api/v1/document_tags_test.rb:31` | PUT without tags clears the map | parité | `rust/parity/tests/api_tags.rs` |  |
| 114 | `test/controllers/api/v1/document_tags_test.rb:37` | keys and values are normalized | parité | `rust/parity/tests/api_tags.rs` |  |
| 115 | `test/controllers/api/v1/document_tags_test.rb:43` | rejects a non-map tags param with 422 and leaves tags untouched | parité | `rust/parity/tests/api_tags.rs` |  |
| 116 | `test/controllers/api/v1/document_tags_test.rb:50` | rejects nested values with 422 | parité | `rust/parity/tests/api_tags.rs` |  |
| 117 | `test/controllers/api/v1/document_tags_test.rb:56` | forbidden for a document outside granted departments | parité | `rust/parity/tests/api_tags.rs` |  |
| 118 | `test/controllers/api/v1/document_tags_test.rb:63` | the tag registry endpoints are gone | parité | `rust/parity/tests/api_tags.rs` |  |
| 119 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:13` | authorized batch across a department and its subtree soft-deletes both | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 120 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:29` | a document outside the client's write access forbids the whole batch | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 121 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:41` | an unknown id mixed with a valid id returns 404 and deletes nothing | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 122 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:51` | an empty document_ids array returns 400 | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 123 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:57` | a missing document_ids param returns 400 | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 124 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:62` | a read-only client is forbidden from bulk-destroying | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 125 | `test/controllers/api/v1/documents_bulk_destroy_test.rb:74` | more than 500 document_ids returns 400 before any existence check | parité | `rust/parity/tests/api_bulk_destroy.rs` |  |
| 126 | `test/controllers/api/v1/documents_test.rb:19` | sync upload returns 201 with the document uuid | parité | `rust/parity/tests/api_documents.rs` |  |
| 127 | `test/controllers/api/v1/documents_test.rb:31` | documents expose a readonly metadata block | parité | `rust/parity/tests/api_documents.rs` |  |
| 128 | `test/controllers/api/v1/documents_test.rb:45` | upload into a subfolder of a granted department is allowed | parité | `rust/parity/tests/api_documents.rs` |  |
| 129 | `test/controllers/api/v1/documents_test.rb:51` | sync upload accepts tags and returns them in the map | parité | `rust/parity/tests/api_documents.rs` |  |
| 130 | `test/controllers/api/v1/documents_test.rb:61` | sync upload with a non-map tags param returns 422 | parité | `rust/parity/tests/api_documents.rs` |  |
| 131 | `test/controllers/api/v1/documents_test.rb:68` | upload with a non-file file param returns 400 | parité | `rust/parity/tests/api_documents.rs` |  |
| 132 | `test/controllers/api/v1/documents_test.rb:75` | upload into a forbidden department returns 403 | parité | `rust/parity/tests/api_documents.rs` |  |
| 133 | `test/controllers/api/v1/documents_test.rb:81` | upload without file returns 400 | parité | `rust/parity/tests/api_documents.rs` |  |
| 134 | `test/controllers/api/v1/documents_test.rb:87` | list returns only accessible documents, with filters | parité | `rust/parity/tests/api_documents.rs` |  |
| 135 | `test/controllers/api/v1/documents_test.rb:109` | documents carry their tags as a map and tag_value filters on the pair | parité | `rust/parity/tests/api_documents.rs` |  |
| 136 | `test/controllers/api/v1/documents_test.rb:129` | tag_value without tag returns 400 | parité | `rust/parity/tests/api_documents.rs` |  |
| 137 | `test/controllers/api/v1/documents_test.rb:135` | list paginates with limit and offset | parité | `rust/parity/tests/api_documents.rs` |  |
| 138 | `test/controllers/api/v1/documents_test.rb:164` | the list is a documents envelope carrying the applied window | parité | `rust/parity/tests/api_documents.rs` |  |
| 139 | `test/controllers/api/v1/documents_test.rb:172` | pagination total reflects the filters, not the whole scope | parité | `rust/parity/tests/api_documents.rs` |  |
| 140 | `test/controllers/api/v1/documents_test.rb:182` | an empty list still carries a pagination block | parité | `rust/parity/tests/api_documents.rs` |  |
| 141 | `test/controllers/api/v1/documents_test.rb:188` | an offset above the cap is echoed clamped | parité | `rust/parity/tests/api_documents.rs` |  |
| 142 | `test/controllers/api/v1/documents_test.rb:194` | departments list paginates too | parité | `rust/parity/tests/api_documents.rs` |  |
| 143 | `test/controllers/api/v1/documents_test.rb:200` | non-scalar limit and offset fall back to the default page size | parité | `rust/parity/tests/api_documents.rs` |  |
| 144 | `test/controllers/api/v1/documents_test.rb:208` | non-string filter params return 400 instead of 500 | parité | `rust/parity/tests/api_documents.rs` |  |
| 145 | `test/controllers/api/v1/documents_test.rb:217` | non-string department_id or callback_url on upload returns 400 | parité | `rust/parity/tests/api_documents.rs` |  |
| 146 | `test/controllers/api/v1/documents_test.rb:229` | filtering by a forbidden department returns 403 | parité | `rust/parity/tests/api_documents.rs` |  |
| 147 | `test/controllers/api/v1/documents_test.rb:235` | download streams the file content | parité | `rust/parity/tests/api_documents.rs` |  |
| 148 | `test/controllers/api/v1/documents_test.rb:242` | show, download, delete of a forbidden document return 403 | parité | `rust/parity/tests/api_documents.rs` |  |
| 149 | `test/controllers/api/v1/documents_test.rb:252` | delete soft-deletes the document, hiding it but keeping the file for the retention window | parité | `rust/parity/tests/api_documents.rs` |  |
| 150 | `test/controllers/api/v1/documents_test.rb:271` | unknown document returns 404 | parité | `rust/parity/tests/api_documents.rs` |  |
| 151 | `test/controllers/api/v1/documents_test.rb:276` | filters documents by owner and upload mode | parité | `rust/parity/tests/api_documents.rs` |  |
| 152 | `test/controllers/api/v1/documents_test.rb:294` | filters documents by uploaded_at, indexed_at and updated_at ranges | parité | `rust/parity/tests/api_documents.rs` |  |
| 153 | `test/controllers/api/v1/documents_test.rb:313` | sync upload targeting trashed department returns 404 | parité | `rust/parity/tests/api_documents.rs` |  |
| 154 | `test/controllers/api/v1/read_write_permissions_test.rb:32` | read-only client can list, show and download | parité | `rust/parity/tests/api_permissions.rs` |  |
| 155 | `test/controllers/api/v1/read_write_permissions_test.rb:44` | read-only client cannot upload | parité | `rust/parity/tests/api_permissions.rs` |  |
| 156 | `test/controllers/api/v1/read_write_permissions_test.rb:51` | read-only client cannot delete | parité | `rust/parity/tests/api_permissions.rs` |  |
| 157 | `test/controllers/api/v1/read_write_permissions_test.rb:57` | read-only client cannot replace tags | parité | `rust/parity/tests/api_permissions.rs` |  |
| 158 | `test/controllers/api/v1/read_write_permissions_test.rb:64` | read-only client cannot create a sub-department | parité | `rust/parity/tests/api_permissions.rs` |  |
| 159 | `test/controllers/api/v1/read_write_permissions_test.rb:72` | write-only client can upload and gets the created document back | parité | `rust/parity/tests/api_permissions.rs` |  |
| 160 | `test/controllers/api/v1/read_write_permissions_test.rb:79` | write-only client cannot list, show or download | parité | `rust/parity/tests/api_permissions.rs` |  |
| 161 | `test/controllers/api/v1/read_write_permissions_test.rb:92` | write-only client cannot filter the list by department | parité | `rust/parity/tests/api_permissions.rs` |  |
| 162 | `test/controllers/api/v1/read_write_permissions_test.rb:98` | write-only client sees no departments in the read-scoped index | parité | `rust/parity/tests/api_permissions.rs` |  |
| 163 | `test/controllers/api/v1/read_write_permissions_test.rb:106` | write-only client can create a sub-department and gets it back | parité | `rust/parity/tests/api_permissions.rs` |  |
| 164 | `test/controllers/api/v1/read_write_permissions_test.rb:115` | write-only client can delete (delete is a write) | parité | `rust/parity/tests/api_permissions.rs` |  |
| 165 | `test/controllers/api/v1/read_write_permissions_test.rb:121` | write-only client can replace tags | parité | `rust/parity/tests/api_permissions.rs` |  |
| 166 | `test/controllers/api/v1/read_write_permissions_test.rb:128` | write-only client cannot poll an upload request (poll is a read) | parité | `rust/parity/tests/api_permissions.rs` |  |
| 167 | `test/controllers/api/v1/read_write_permissions_test.rb:143` | destroy requires write even though it shares the finder with show | parité | `rust/parity/tests/api_permissions.rs` |  |
| 168 | `test/controllers/api/v1/upload_requests_test.rb:11` | GET /api/v1/upload_requests/:id for trashed dept returns 404 | parité | `rust/parity/tests/api_async_upload.rs` |  |
| 169 | `test/controllers/api/v1/upload_requests_test.rb:28` | GET /api/v1/upload_requests/:id for live dept returns 200 | parité | `rust/parity/tests/api_async_upload.rs` |  |
| 170 | `test/initializers/oidc_config_test.rb:11` | verify! raises in production when GED_OIDC_AUDIENCE is absent | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 171 | `test/initializers/oidc_config_test.rb:18` | verify! passes in production when all required vars are present | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 172 | `test/initializers/oidc_config_test.rb:24` | verify! never raises in the test environment even if a var is missing | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 173 | `test/initializers/oidc_config_test.rb:32` | verify! raises in production when scope binding lacks GED_OIDC_REQUIRED_SCOPE | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 174 | `test/initializers/oidc_config_test.rb:39` | verify! passes in production when scope binding has GED_OIDC_REQUIRED_SCOPE | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 175 | `test/initializers/oidc_config_test.rb:46` | verify! raises in production for an unknown GED_OIDC_TOKEN_BINDING value | unitaire Rust | `rust/ged/src/config.rs` | refus de boot fail-closed |
| 176 | `test/integration/api_docs_test.rb:10` | GET /api/v1/openapi.yaml serves the spec without authentication | parité | `rust/parity/tests/api_docs.rs` |  |
| 177 | `test/integration/api_docs_test.rb:19` | GET /api/docs renders the Redoc page pointing at the spec | parité | `rust/parity/tests/api_docs.rs` |  |
| 178 | `test/integration/api_docs_test.rb:28` | every routed /api/v1 path is documented in the spec | unitaire Rust | `rust/ged/tests/openapi_paths.rs` |  |
| 179 | `test/integration/api_docs_test.rb:44` | documents list response conforms to envelope schema | parité | `rust/parity/tests/api_docs.rs` |  |
| 180 | `test/integration/c4_diagrams_test.rb:17` | the diagram file exists and every mermaid block is closed | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 181 | `test/integration/c4_diagrams_test.rb:25` | every docker compose service appears in the container diagram | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 182 | `test/integration/c4_diagrams_test.rb:34` | every ActiveJob appears in the worker component diagram | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 183 | `test/integration/c4_diagrams_test.rb:44` | every service object appears in a component diagram | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 184 | `test/integration/c4_diagrams_test.rb:54` | every recurring task class is drawn as scheduled | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 185 | `test/integration/c4_diagrams_test.rb:63` | every solid queue pool is documented | unitaire Rust | `rust/ged/tests/c4_diagrams.rs` |  |
| 186 | `test/integration/health_check_test.rb:4` | GET /up returns 200 | parité | `rust/parity/tests/health.rs` |  |
| 187 | `test/jobs/deliver_callback_job_test.rb:19` | posts the payload to the callback url | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 188 | `test/jobs/deliver_callback_job_test.rb:27` | includes the error message for failed requests | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 189 | `test/jobs/deliver_callback_job_test.rb:39` | raises DeliveryFailed on non-2xx so retry_on kicks in | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 190 | `test/jobs/deliver_callback_job_test.rb:46` | raises DeliveryFailed on connection errors | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 191 | `test/jobs/deliver_callback_job_test.rb:53` | raises DeliveryFailed on connection reset | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 192 | `test/jobs/deliver_callback_job_test.rb:60` | raises DeliveryFailed on SSL errors | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 193 | `test/jobs/deliver_callback_job_test.rb:67` | raises DeliveryFailed on unreachable host or network | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 194 | `test/jobs/deliver_callback_job_test.rb:74` | raises DeliveryFailed on timeout | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 195 | `test/jobs/deliver_callback_job_test.rb:81` | refuses a callback host resolving to a private address, without retrying | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 196 | `test/jobs/deliver_callback_job_test.rb:89` | refuses an IP-literal loopback callback slipped past validation | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 197 | `test/jobs/deliver_callback_job_test.rb:97` | delivers normally when the host resolves publicly | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 198 | `test/jobs/deliver_callback_job_test.rb:105` | emits outcome=success on delivery | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 199 | `test/jobs/deliver_callback_job_test.rb:113` | emits outcome=failure on a failed attempt | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 200 | `test/jobs/deliver_callback_job_test.rb:123` | emits outcome=failure on a connection error | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 201 | `test/jobs/deliver_callback_job_test.rb:133` | emits outcome=refused for a blocked callback target | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 202 | `test/jobs/deliver_callback_job_test.rb:141` | emits outcome=exhausted once retries run out | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` |  |
| 203 | `test/jobs/purge_deleted_documents_job_test.rb:18` | hard-deletes documents past their purge_at and purges their file | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 204 | `test/jobs/purge_deleted_documents_job_test.rb:32` | keeps soft-deleted documents still within the retention window | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 205 | `test/jobs/purge_deleted_documents_job_test.rb:39` | keeps live documents | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 206 | `test/jobs/purge_deleted_documents_job_test.rb:45` | hard-deletes a soft-deleted leaf department once its documents are purged | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 207 | `test/jobs/purge_deleted_documents_job_test.rb:63` | keeps a soft-deleted department when a trashed document still remains | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 208 | `test/jobs/purge_deleted_documents_job_test.rb:78` | keeps a soft-deleted department when an UploadRequest references it | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 209 | `test/jobs/purge_deleted_documents_job_test.rb:96` | never touches active (non-trashed) departments | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 210 | `test/jobs/purge_deleted_documents_job_test.rb:107` | hard-delete of department also destroys its access grants | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 211 | `test/jobs/purge_deleted_documents_job_test.rb:127` | purges trashed leaf department with live parent | parité | `rust/parity/tests/security_purge.rs` | purge_at avancé en SQL puis Backend::run_job ; lignes métier seulement (pas de lecture Active Storage) |
| 212 | `test/jobs/queue_coverage_test.rb:8` | every job queue is drained by a worker pool in every environment | unitaire Rust | `rust/ged/src/jobs/queue.rs` |  |
| 213 | `test/jobs/reconcile_upload_requests_job_test.rb:16` | re-enqueues StoreDocumentJob for stuck pending requests with a staging file | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 214 | `test/jobs/reconcile_upload_requests_job_test.rb:26` | fails stuck pending requests whose staging file is gone and enqueues the callback | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 215 | `test/jobs/reconcile_upload_requests_job_test.rb:35` | leaves fresh pending requests alone | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 216 | `test/jobs/reconcile_upload_requests_job_test.rb:40` | resets stuck processing requests with a staging file and re-enqueues | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 217 | `test/jobs/reconcile_upload_requests_job_test.rb:52` | fails stuck processing requests whose staging file is gone and enqueues the callback | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 218 | `test/jobs/reconcile_upload_requests_job_test.rb:61` | leaves fresh processing requests alone | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 219 | `test/jobs/reconcile_upload_requests_job_test.rb:66` | purges terminal requests older than the retention window | parité | `rust/parity/tests/jobs_reconcile.rs` | ligne upload_requests fixturée en SQL puis run_job |
| 220 | `test/jobs/reconcile_upload_requests_job_test.rb:74` | purges unattached blobs older than a day, keeps fresh and attached ones | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 221 | `test/jobs/reconcile_upload_requests_job_test.rb:97` | purges staging files older than the retention window | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 222 | `test/jobs/reconcile_upload_requests_job_test.rb:109` | does not re-enqueue stuck pending requests while the uploads queue is paused | unitaire Rust | `rust/ged/src/jobs/reconcile.rs` | staging/blobs hors fixtures |
| 223 | `test/jobs/store_document_job_test.rb:13` | stores the document, completes the request, enqueues the callback | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 224 | `test/jobs/store_document_job_test.rb:22` | records the original upload time and marks the document async | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 225 | `test/jobs/store_document_job_test.rb:32` | applies the requested tags to the created document | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 226 | `test/jobs/store_document_job_test.rb:44` | concurrent double execution stores the document only once | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 227 | `test/jobs/store_document_job_test.rb:69` | leaves no document behind when the request cannot be marked completed | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 228 | `test/jobs/store_document_job_test.rb:94` | marks the request failed on error and still enqueues the callback | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 229 | `test/jobs/store_document_job_test.rb:104` | runs on the dedicated uploads queue | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 230 | `test/jobs/store_document_job_test.rb:108` | async fail-closed: soft-deleted department fails the job | unitaire Rust | `rust/ged/src/jobs/store_document.rs` | transaction unique, crash entre écritures |
| 231 | `test/lib/telemetry_test.rb:7` | the OpenTelemetry SDK is not loaded when no OTLP endpoint is configured | unitaire Rust | `rust/ged/src/telemetry.rs` |  |
| 232 | `test/lib/telemetry_test.rb:11` | instruments are no-ops that accept adds and records without raising | unitaire Rust | `rust/ged/src/telemetry.rs` |  |
| 233 | `test/lib/telemetry_test.rb:19` | instruments are memoized | unitaire Rust | `rust/ged/src/telemetry.rs` |  |
| 234 | `test/models/admin_test.rb:4` | requires unique username | unitaire Rust | `rust/ged/src/admin/session.rs` |  |
| 235 | `test/models/admin_test.rb:10` | authenticates with valid password | unitaire Rust | `rust/ged/src/admin/session.rs` |  |
| 236 | `test/models/app_client_test.rb:12` | client_identifier is required and unique | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 237 | `test/models/app_client_test.rb:17` | can_access? covers the granted department and its subtree for both modes | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 238 | `test/models/app_client_test.rb:26` | a read-only grant does not allow write | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 239 | `test/models/app_client_test.rb:35` | an unknown mode fails closed instead of falling through to read | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 240 | `test/models/app_client_test.rb:48` | a write-only grant does not allow read | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 241 | `test/models/app_client_test.rb:56` | read and write cascade independently, child grants never escalate upward | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 242 | `test/models/app_client_test.rb:66` | accessible_department_ids expands granted subtrees per mode | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 243 | `test/models/client_department_access_test.rb:9` | defaults to full read/write access | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 244 | `test/models/client_department_access_test.rb:15` | rejects a grant with neither read nor write | unitaire Rust | `rust/ged/src/domain/app_client.rs` |  |
| 245 | `test/models/department_test.rb:4` | requires a name, unique among siblings | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 246 | `test/models/department_test.rb:11` | self_and_ancestors walks up to the root | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 247 | `test/models/department_test.rb:18` | subtree_ids includes self and all descendants | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 248 | `test/models/department_test.rb:27` | destroying a department cascades its client access grants | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 249 | `test/models/department_test.rb:35` | destroy is blocked when upload requests reference the department | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 250 | `test/models/department_test.rb:45` | parent cannot be self or a descendant | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 251 | `test/models/department_test.rb:63` | soft_delete! sets deleted_at on the department and trashes its live documents | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 252 | `test/models/department_test.rb:83` | restore! clears deleted_at and restores all trashed documents | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 253 | `test/models/department_test.rb:105` | default_scope hides soft-deleted departments | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 254 | `test/models/department_test.rb:122` | leaf? returns false when department has a live child | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 255 | `test/models/department_test.rb:130` | soft_delete! raises when department has live children | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 256 | `test/models/department_test.rb:140` | soft_delete! raises when department has trashed children | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 257 | `test/models/department_test.rb:154` | name uniqueness validation allows restoring a trashed department's name | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 258 | `test/models/department_test.rb:169` | .tree returns depth-first pairs with depths, filtered sets promoting orphans to roots | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 259 | `test/models/department_test.rb:177` | clear_soft_delete! un-greys the department without restoring documents | unitaire Rust | `rust/ged/src/domain/department.rs` |  |
| 260 | `test/models/document_test.rb:17` | has a uuid primary key and stores a file | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 261 | `test/models/document_test.rb:24` | requires filename, department, uploaded_by | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 262 | `test/models/document_test.rb:31` | can be tagged | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 263 | `test/models/document_test.rb:38` | defaults uploaded_at to creation time and is sync | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 264 | `test/models/document_test.rb:48` | keeps an explicitly provided uploaded_at and async flag | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 265 | `test/models/document_test.rb:60` | metadata_hash exposes uploaded_at, updated_at and upload_mode | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 266 | `test/models/document_test.rb:70` | soft_delete! records deleted_at and a purge_at 30 days later | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 267 | `test/models/document_test.rb:78` | default scope hides soft-deleted documents | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 268 | `test/models/document_test.rb:87` | deleted and with_deleted scopes reach soft-deleted documents | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 269 | `test/models/document_test.rb:97` | purgeable returns only soft-deleted documents past their purge_at | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 270 | `test/models/document_test.rb:110` | restore! clears the soft-delete columns and makes the document visible again | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 271 | `test/models/tags_map_validation_test.rb:12` | accepts a flat map of string keys with string or null values | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 272 | `test/models/tags_map_validation_test.rb:17` | normalizes keys (strip+downcase) and values (strip, blank becomes null) | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 273 | `test/models/tags_map_validation_test.rb:22` | symbol keys are stringified | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 274 | `test/models/tags_map_validation_test.rb:26` | rejects values that are not a map | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 275 | `test/models/tags_map_validation_test.rb:34` | rejects nested or non-string values | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 276 | `test/models/tags_map_validation_test.rb:40` | rejects blank and overlong keys, and overlong values | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 277 | `test/models/tags_map_validation_test.rb:46` | rejects more than 50 entries | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 278 | `test/models/tags_map_validation_test.rb:51` | upload requests validate tags the same way | unitaire Rust | `rust/ged/src/domain/tags.rs` |  |
| 279 | `test/models/upload_request_test.rb:4` | deleting a document nulls the referencing upload_request's document_id | unitaire Rust | `rust/ged/src/domain/upload_request.rs` |  |
| 280 | `test/models/upload_request_test.rb:20` | rejects a callback_url targeting a private or internal IP literal | unitaire Rust | `rust/ged/src/domain/upload_request.rs` |  |
| 281 | `test/models/upload_request_test.rb:30` | rejects a non-http callback_url | unitaire Rust | `rust/ged/src/domain/upload_request.rs` |  |
| 282 | `test/models/upload_request_test.rb:38` | accepts a public hostname callback_url without resolving DNS | unitaire Rust | `rust/ged/src/domain/upload_request.rs` |  |
| 283 | `test/security/auth_matrix_test.rb:14` | no Authorization header returns 401 | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 284 | `test/security/auth_matrix_test.rb:19` | garbage bearer token returns 401 | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 285 | `test/security/auth_matrix_test.rb:24` | alg:none forged token is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 286 | `test/security/auth_matrix_test.rb:34` | HS256 algorithm-confusion token signed with the RSA public key is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 287 | `test/security/auth_matrix_test.rb:45` | tampered payload (bit flipped after signing) is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 288 | `test/security/auth_matrix_test.rb:56` | stripped signature is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 289 | `test/security/auth_matrix_test.rb:64` | expired token is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 290 | `test/security/auth_matrix_test.rb:70` | token with a future nbf is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 291 | `test/security/auth_matrix_test.rb:82` | wrong issuer is rejected | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 292 | `test/security/auth_matrix_test.rb:88` | valid token for a registered client returns 200 | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 293 | `test/security/auth_matrix_test.rb:93` | valid token for an unregistered client returns 403, not 401 | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 294 | `test/security/auth_matrix_test.rb:101` | token with a foreign audience is rejected when GED_OIDC_AUDIENCE is set | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 295 | `test/security/auth_matrix_test.rb:118` | authentication failures return a generic error body, not jwt-gem internals | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 296 | `test/security/auth_matrix_test.rb:129` | a malicious kid is not reflected in the error body | parité | `rust/parity/tests/auth_matrix.rs` | porté (T1) ; jetons forgés signés avec la clé de l'oauth-provider |
| 297 | `test/security/authz_idor_test.rb:43` | deptA client cannot show a document in dept B | parité | `rust/parity/tests/security_authz.rs` |  |
| 298 | `test/security/authz_idor_test.rb:48` | deptA client cannot download a document in dept B | parité | `rust/parity/tests/security_authz.rs` |  |
| 299 | `test/security/authz_idor_test.rb:53` | deptA client cannot delete a document in dept B | parité | `rust/parity/tests/security_authz.rs` |  |
| 300 | `test/security/authz_idor_test.rb:59` | deptA client cannot replace tags on a document in dept B | parité | `rust/parity/tests/security_authz.rs` |  |
| 301 | `test/security/authz_idor_test.rb:66` | deptA client cannot filter the list by dept B's department_id | parité | `rust/parity/tests/security_authz.rs` |  |
| 302 | `test/security/authz_idor_test.rb:72` | deptA client's unfiltered list never leaks dept B's document | parité | `rust/parity/tests/security_authz.rs` |  |
| 303 | `test/security/authz_idor_test.rb:80` | read/write grant on a department cascades to its child (upload) | parité | `rust/parity/tests/security_authz.rs` |  |
| 304 | `test/security/authz_idor_test.rb:86` | a grant on a child department never escalates to reach the parent's document | parité | `rust/parity/tests/security_authz.rs` |  |
| 305 | `test/security/authz_idor_test.rb:91` | write-only client cannot read (write does not imply read) | parité | `rust/parity/tests/security_authz.rs` |  |
| 306 | `test/security/authz_idor_test.rb:96` | a client with no grants cannot access documents at all | parité | `rust/parity/tests/security_authz.rs` |  |
| 307 | `test/security/authz_idor_test.rb:105` | a random unknown document UUID is 404, never 200 or 500 | parité | `rust/parity/tests/security_authz.rs` |  |
| 308 | `test/security/authz_idor_test.rb:110` | deptA client cannot poll an upload request that belongs to dept B | parité | `rust/parity/tests/security_authz.rs` |  |
| 309 | `test/security/input_fuzz_test.rb:30` | a nested hash where a scalar param is expected returns 400, not 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 310 | `test/security/input_fuzz_test.rb:36` | an array where a scalar param is expected returns 400, not 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 311 | `test/security/input_fuzz_test.rb:44` | tag_value without tag returns 400 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 312 | `test/security/input_fuzz_test.rb:50` | an unparsable date filter returns 400 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 313 | `test/security/input_fuzz_test.rb:56` | an invalid upload_mode returns 400 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 314 | `test/security/input_fuzz_test.rb:64` | a negative limit is clamped, not a 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 315 | `test/security/input_fuzz_test.rb:69` | a huge limit is clamped, not a 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 316 | `test/security/input_fuzz_test.rb:74` | a negative offset is clamped, not a 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 317 | `test/security/input_fuzz_test.rb:79` | a pagination offset beyond bigint range is handled, not a 500 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 318 | `test/security/input_fuzz_test.rb:95` | more than 50 tag entries is rejected with 422 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 319 | `test/security/input_fuzz_test.rb:102` | a tag key longer than 100 characters is rejected with 422 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 320 | `test/security/input_fuzz_test.rb:109` | a tag value longer than 500 characters is rejected with 422 | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 321 | `test/security/input_fuzz_test.rb:118` | malformed JSON body returns an app-level JSON 400 error | parité | `rust/parity/tests/security_fuzz.rs` |  |
| 322 | `test/security/input_fuzz_test.rb:139` | a hostile filename is sanitized before being persisted | parité | `rust/parity/tests/security_fuzz.rs` | filename lu dans le JSON de création (colonne documents), pas le blob |
| 323 | `test/security/input_fuzz_test.rb:170` | an oversized upload is rejected | parité | `rust/parity/tests/security_fuzz.rs` | 413 corps vide au bord (Thruster), pas le 422 modèle ; voir parity-gaps.md |
| 324 | `test/security/input_fuzz_test.rb:184` | a legitimate filename with a space survives sanitization unchanged | parité | `rust/parity/tests/security_fuzz.rs` | filename lu dans le JSON de création |
| 325 | `test/security/input_fuzz_test.rb:197` | a backslash path filename is reduced to its basename | parité | `rust/parity/tests/security_fuzz.rs` | filename lu dans le JSON de création |
| 326 | `test/security/input_fuzz_test.rb:210` | a filename that sanitizes to empty falls back to unnamed | parité | `rust/parity/tests/security_fuzz.rs` | filename lu dans le JSON de création |
| 327 | `test/security/input_fuzz_test.rb:225` | a byte_size of nil stays valid (guards allow_nil) | unitaire Rust | `rust/ged/src/domain/document.rs` |  |
| 328 | `test/security/input_fuzz_test.rb:238` | a sync oversized upload is rejected with 422 | parité | `rust/parity/tests/security_fuzz.rs` | 413 corps vide au bord (Thruster), pas le 422 modèle ; voir parity-gaps.md |
| 329 | `test/security/input_fuzz_test.rb:248` | an async oversized upload is rejected with 422 AND the staging file is never written | unitaire Rust | `rust/ged/src/storage/staging.rs` | 413 au bord et absence de ligne couverts par security_fuzz.rs ; le fichier de staging reste non observable en HTTP |
| 330 | `test/security/ssrf_test.rb:16` | decimal-encoded loopback IP is blocked | parité | `rust/parity/tests/security_ssrf.rs` | 422 sur callback_url numérique ; l'encodage lui-même est testé en unitaire |
| 331 | `test/security/ssrf_test.rb:21` | hex-encoded loopback IP is blocked | parité | `rust/parity/tests/security_ssrf.rs` | 422 sur callback_url numérique ; l'encodage lui-même est testé en unitaire |
| 332 | `test/security/ssrf_test.rb:26` | zero-padded octal-encoded loopback IP is blocked | parité | `rust/parity/tests/security_ssrf.rs` | 422 sur callback_url numérique ; l'encodage lui-même est testé en unitaire |
| 333 | `test/security/ssrf_test.rb:34` | short-form (127.1) loopback IP is blocked | parité | `rust/parity/tests/security_ssrf.rs` | 422 sur callback_url numérique ; l'encodage lui-même est testé en unitaire |
| 334 | `test/security/ssrf_test.rb:41` | a real hostname is not mis-parsed as a numeric IP | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 335 | `test/security/ssrf_test.rb:49` | an out-of-range dotted-quad is not mis-parsed as a numeric IP | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 336 | `test/security/ssrf_test.rb:58` | the callback connects to the pinned address that passed the SSRF check | unitaire Rust | `rust/ged/src/jobs/deliver_callback.rs` | pinning d'IP, non observable en HTTP |
| 337 | `test/security/ssrf_test.rb:96` | an unresolvable host is refused (nothing to pin, GED-SEC-10 reversed) | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 338 | `test/seeds_test.rb:8` | re-seeding updates the admin password when GED_ADMIN_PASSWORD changes | unitaire Rust | `rust/ged/src/main.rs` | sous-commande seed |
| 339 | `test/services/auth/jwks_cache_test.rb:4` | serves keys from cache without re-fetching until the TTL expires | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 340 | `test/services/auth/jwks_cache_test.rb:10` | backs off instead of re-fetching on every call while the IdP is down | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 341 | `test/services/auth/jwks_cache_test.rb:21` | fails closed without hammering the IdP when keys were never fetched | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 342 | `test/services/auth/jwks_cache_test.rb:29` | health reports a never-fetched cache without triggering a fetch | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 343 | `test/services/auth/jwks_cache_test.rb:37` | health reports a healthy cache after a successful fetch | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 344 | `test/services/auth/jwks_cache_test.rb:47` | health flags a cache serving stale keys after a failed refresh | unitaire Rust | `rust/ged/src/auth/jwks_cache.rs` |  |
| 345 | `test/services/auth/token_validator_test.rb:4` | returns the client identifier from a valid token | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 346 | `test/services/auth/token_validator_test.rb:10` | rejects an expired token | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 347 | `test/services/auth/token_validator_test.rb:16` | rejects a token with the wrong issuer | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 348 | `test/services/auth/token_validator_test.rb:22` | rejects a token signed by an unknown key | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 349 | `test/services/auth/token_validator_test.rb:30` | fails closed when JWKS was never fetched | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 350 | `test/services/auth/token_validator_test.rb:37` | recovers from IdP key rotation by re-fetching JWKS on unknown kid | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 351 | `test/services/auth/token_validator_test.rb:53` | unknown kid forces at most one JWKS re-fetch per rate-limit window | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 352 | `test/services/auth/token_validator_test.rb:67` | reads the client identifier from the claim named by GED_CLIENT_ID_CLAIM | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 353 | `test/services/auth/token_validator_test.rb:86` | verifies the aud claim when GED_OIDC_AUDIENCE is set | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 354 | `test/services/auth/token_validator_test.rb:116` | keeps serving stale keys when a refresh fails | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 355 | `test/services/auth/token_validator_test.rb:125` | scope binding accepts a token whose scope claim string contains the required scope | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 356 | `test/services/auth/token_validator_test.rb:134` | scope binding accepts a token whose scp claim array contains the required scope | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 357 | `test/services/auth/token_validator_test.rb:143` | scope binding rejects a token missing the required scope | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 358 | `test/services/auth/token_validator_test.rb:154` | scope binding does not verify aud | unitaire Rust | `rust/ged/src/auth/token_validator.rs` |  |
| 359 | `test/services/callback_url_policy_test.rb:4` | accepts a public hostname without resolving | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 360 | `test/services/callback_url_policy_test.rb:8` | accepts a public IP literal | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 361 | `test/services/callback_url_policy_test.rb:12` | rejects non-http(s) or hostless URLs | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 362 | `test/services/callback_url_policy_test.rb:19` | rejects blocked IP literals without touching DNS | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 363 | `test/services/callback_url_policy_test.rb:38` | resolve: false never queries DNS | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 364 | `test/services/callback_url_policy_test.rb:44` | resolve: true rejects a hostname resolving to a blocked address | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 365 | `test/services/callback_url_policy_test.rb:51` | resolve: true rejects when any resolved address is blocked | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 366 | `test/services/callback_url_policy_test.rb:57` | resolve: true accepts a hostname resolving publicly | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 367 | `test/services/callback_url_policy_test.rb:63` | resolve: true refuses an unresolvable host (nothing to pin) | unitaire Rust | `rust/ged/src/policy/callback_url.rs` |  |
| 368 | `test/services/disk_space_test.rb:4` | reports total and available space for an existing path | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 369 | `test/services/disk_space_test.rb:11` | returns nil for a nonexistent path | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 370 | `test/services/document_creator_test.rb:10` | emits a documents_uploaded count with upload_mode=sync | unitaire Rust | `rust/ged/src/domain/document.rs` | émission de métriques |
| 371 | `test/services/document_creator_test.rb:17` | emits upload_mode=async for async-stored documents | unitaire Rust | `rust/ged/src/domain/document.rs` | émission de métriques |
| 372 | `test/services/document_creator_test.rb:24` | emits nothing when the document is invalid | unitaire Rust | `rust/ged/src/domain/document.rs` | émission de métriques |
| 373 | `test/services/system_health_test.rb:4` | database check is ok when the database answers | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 374 | `test/services/system_health_test.rb:9` | job workers check is ok when a worker heartbeat is fresh | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 375 | `test/services/system_health_test.rb:16` | job workers check is bad when all heartbeats are stale | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 376 | `test/services/system_health_test.rb:23` | job workers check is bad when no worker is registered | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 377 | `test/services/system_health_test.rb:29` | jwks check is unknown before any fetch and does no HTTP | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 378 | `test/services/system_health_test.rb:35` | jwks check is ok after a successful fetch | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 379 | `test/services/system_health_test.rb:41` | jwks check warns when serving stale keys after a failed refresh | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 380 | `test/services/system_health_test.rb:51` | storage disk check maps used fraction to thresholds | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 381 | `test/services/system_health_test.rb:60` | storage disk check is unknown when df is unavailable | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 382 | `test/services/system_health_test.rb:66` | checks returns database, workers, jwks and disk in display order | unitaire Rust | `rust/ged/src/admin/status.rs` |  |
| 383 | `test/system/admin_ui_test.rb:20` | 2.1 login with valid credentials lands on the documents list | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 384 | `test/system/admin_ui_test.rb:27` | 2.2 login with a wrong password is rejected and creates no session | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 385 | `test/system/admin_ui_test.rb:42` | 2.3 create departments with a hierarchy | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 386 | `test/system/admin_ui_test.rb:57` | 2.4 a duplicate department at the same level is rejected | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 387 | `test/system/admin_ui_test.rb:67` | department tree rows collapse and expand | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 388 | `test/system/admin_ui_test.rb:84` | 2.5 tag keys are normalized to lowercase in the document tags textarea | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 389 | `test/system/admin_ui_test.rb:104` | 2.6 upload a document and see it in the list attributed to the admin | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 390 | `test/system/admin_ui_test.rb:120` | 2.7 download the uploaded document returns the original bytes | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 391 | `test/system/admin_ui_test.rb:130` | 2.8 filter the document list by filename and by department | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 392 | `test/system/admin_ui_test.rb:156` | 2.9 delete a document removes it from the list | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 393 | `test/system/admin_ui_test.rb:168` | 2.9b a deleted document can be restored from the trash | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 394 | `test/system/admin_ui_test.rb:188` | clicking a document row opens the split-pane preview in an iframe | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 395 | `test/system/admin_ui_test.rb:209` | an image row renders an <img> preview, not an iframe | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 396 | `test/system/admin_ui_test.rb:223` | a non-previewable content type shows the fallback card instead of an iframe | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 397 | `test/system/admin_ui_test.rb:237` | the Close button hides the preview and restores the full-width table | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 398 | `test/system/admin_ui_test.rb:252` | clicking Download/Tags/Delete in a row does not open the preview | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 399 | `test/system/admin_ui_test.rb:265` | select-all enables the bulk toolbar and Delete selected trashes the selection | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 400 | `test/system/admin_ui_test.rb:287` | bulk ZIP download contains the selected documents | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 401 | `test/system/admin_ui_test.rb:309` | 2.10 deleting a department with documents offers move or trash, then trashes | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 402 | `test/system/admin_ui_test.rb:343` | 2.11 after logout a protected page redirects to login | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 403 | `test/system/admin_ui_test.rb:354` | the status page shows health checks and key metrics | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
| 404 | `test/system/admin_ui_test.rb:367` | the light/dark toggle switches the theme and persists across reloads | navigateur | `rust/e2e/tests/admin_ui.spec.ts` | porté T3b |
