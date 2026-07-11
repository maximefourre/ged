# GED — Plan de tests QA

Toutes les commandes de ce document ont été vérifiées le 2026-07-10 sur la
stack Docker Compose complète (macOS, Docker 29.x). Prérequis : Docker
installé, le dépôt cloné, un terminal à la racine du projet.

Vocabulaire :
- **Département** = dossier = frontière de permission. Un client qui a accès à
  un département a accès à tout son sous-arbre (sous-dossiers).
- **Client applicatif** = un système qui appelle l'API avec un token OAuth2
  (client_credentials). Il faut l'enregistrer **deux fois** : dans le
  `oauth-provider` (pour obtenir des credentials) ET dans la GED (pour lui
  donner des droits sur des départements).

---

## 1. Lancer l'application

Créer le fichier `.env` à la racine (une seule fois — il est lu automatiquement
par toutes les commandes `docker compose`) :

```bash
printf 'SECRET_KEY_BASE=%s\nGED_ADMIN_PASSWORD=changeme\n' "$(openssl rand -hex 64)" > .env
```

Démarrer la stack (premier lancement : le build prend quelques minutes) :

```bash
docker compose up --build -d
```

Créer le compte admin (premier lancement uniquement) :

```bash
docker compose exec web bin/rails db:seed
```

**Vérifications attendues :**

| Contrôle | Commande | Attendu |
|---|---|---|
| L'app répond | `curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/up` | `200` |
| Le provider OAuth répond | `curl -s http://localhost:9292/.well-known/jwks.json` | JSON avec `"keys":[...]` |
| Page de login admin | ouvrir http://localhost:3000 dans un navigateur | redirection vers la page de connexion |

---

## 2. Interface d'administration (navigateur)

URL : http://localhost:3000 — identifiants : `admin` / `changeme` (la valeur
de `GED_ADMIN_PASSWORD` dans `.env`).

Scénarios à dérouler :

| # | Scénario | Attendu |
|---|---|---|
| 2.1 | Connexion avec `admin`/`changeme` | Arrivée sur la liste des documents |
| 2.2 | Connexion avec un mauvais mot de passe | Erreur, pas de session |
| 2.3 | Departments → créer `Finance` (sans parent), puis `Invoices` avec parent `Finance`, puis `HR` | Les 3 apparaissent dans la liste avec la bonne hiérarchie |
| 2.4 | Créer un doublon `Finance` au même niveau | Message d'erreur, pas de création |
| 2.5 | Documents → un document → "Tags", saisir `Type=Facture` puis `urgent` (une paire `clé=valeur` par ligne, une clé seule = valeur nulle) | Colonne Tags = `type=Facture, urgent` (clé normalisée en minuscules, valeur conservée) |
| 2.6 | Documents → Upload d'un fichier dans `Finance` | Le document apparaît dans la liste, colonne "Uploaded by" = `admin:admin` |
| 2.7 | Documents → Download du fichier uploadé | Le fichier téléchargé est identique à l'original |
| 2.8 | Documents → filtre par nom de fichier et par département | La liste se restreint correctement |
| 2.9 | Documents → Delete | Le document disparaît |
| 2.10 | Supprimer un département qui contient des documents | Refus avec message d'erreur |
| 2.11 | Log out puis accès direct à http://localhost:3000/admin/documents | Redirection vers la page de login |
| 2.12 | Upload requests → bouton "Pause" | Le bandeau passe à `paused`, un "Resume" apparaît ; les uploads async en file ne sont plus dépilés (le job en cours finit et son callback part quand même) |
| 2.13 | Upload requests → "Resume" | Le bandeau repasse à `active`, la file se vide |

---

## 3. Enregistrer un client applicatif (préalable aux tests API)

**Étape A — côté provider OAuth** (fournit les credentials) :

```bash
docker compose exec oauth-provider bin/register-client qa-app
```

→ Affiche `client_id` et `client_secret`. **Noter le secret immédiatement**,
il n'est pas récupérable ensuite.

**Étape B — côté GED** (donne les droits) : dans l'admin UI → Clients →
"Register client" → client_identifier : `qa-app`, cocher le département
`Finance` uniquement (pas HR — on veut tester le refus).

---

## 4. Obtenir un token

```bash
curl -s -X POST http://localhost:9292/token \
  -d grant_type=client_credentials \
  -d client_id=qa-app \
  -d client_secret=<LE_SECRET_NOTÉ>
```

**Attendu :** JSON `{ "access_token": "...", "token_type": "Bearer",
"expires_in": 3600 }`. Le token est valable **1 heure**.

Pour la suite, l'exporter :

```bash
TOKEN=$(curl -s -X POST http://localhost:9292/token \
  -d grant_type=client_credentials -d client_id=qa-app \
  -d client_secret=<LE_SECRET_NOTÉ> | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')
```

Cas d'erreur à tester sur `/token` :

| # | Cas | Attendu |
|---|---|---|
| 4.1 | Mauvais `client_secret` | `401` `{"error":"invalid_client"}` |
| 4.2 | `client_id` inconnu | `401` `{"error":"invalid_client"}` |
| 4.3 | `grant_type=password` | `400` `{"error":"unsupported_grant_type"}` |

---

## 5. Authentification API — la progression 401 → 403 → 200

C'est le test clé du modèle de sécurité. Endpoint de sonde : `GET /api/v1/ping`.

| # | Cas | Commande | Attendu |
|---|---|---|---|
| 5.1 | Sans token | `curl -s -w " [%{http_code}]" http://localhost:3000/api/v1/ping` | `401` `{"error":"missing bearer token"}` |
| 5.2 | Token bidon | `curl -s -w " [%{http_code}]" http://localhost:3000/api/v1/ping -H "Authorization: Bearer nimporte"` | `401` |
| 5.3 | Token valide mais client **non** enregistré côté GED (utiliser un client enregistré uniquement à l'étape 3-A) | idem avec `$TOKEN` | `403` `{"error":"client not registered"}` |
| 5.4 | Token valide + client enregistré côté GED | idem | `200` `{"client":"qa-app"}` |
| 5.5 | Token expiré (attendre 1 h, ou garder un vieux token) | idem | `401` |

---

## 6. API Documents

Récupérer d'abord les IDs de départements (visibles dans l'admin UI, ou via la
liste API ci-dessous). Dans les exemples : `Finance=1`, `HR=3`.

```bash
printf 'contenu de test' > /tmp/qa-doc.pdf
```

| # | Cas | Commande | Attendu |
|---|---|---|---|
| 6.1 | **Upload synchrone** dans Finance | `curl -s -w " [%{http_code}]" -X POST http://localhost:3000/api/v1/documents -H "Authorization: Bearer $TOKEN" -F "department_id=1" -F "file=@/tmp/qa-doc.pdf"` | `201`, JSON avec `id` (UUID), `filename`, `uploaded_by:"qa-app"` |
| 6.2 | Upload dans un **sous-dossier** de Finance (Invoices) | idem avec `department_id=2` | `201` (l'accès à Finance couvre son sous-arbre) |
| 6.3 | Upload dans **HR** (non autorisé) | idem avec `department_id=3` | `403` `{"error":"no access to this department"}` |
| 6.4 | Upload **sans fichier** | idem sans `-F file=...` | `400` |
| 6.5 | **Lister** | `curl -s http://localhost:3000/api/v1/documents -H "Authorization: Bearer $TOKEN"` | Uniquement les documents des départements accessibles |
| 6.6 | Lister avec filtres | `...?filename=qa`, `...?tag=type`, `...?tag=type&tag_value=facture`, `...?department_id=1` | Liste filtrée (`tag` = présence de la clé, `+tag_value` = paire exacte) ; `department_id=3` → `403` ; `tag_value` seul → `400` |
| 6.7 | **Télécharger** | `curl -s http://localhost:3000/api/v1/documents/<UUID>/download -H "Authorization: Bearer $TOKEN"` | Contenu identique à l'original |
| 6.8 | **Supprimer** | `curl -s -X DELETE http://localhost:3000/api/v1/documents/<UUID> -H "Authorization: Bearer $TOKEN"` | `204` ; re-GET → `404` |
| 6.9 | Accéder à un document d'HR (créé via l'admin UI) | GET/download/DELETE avec son UUID | `403` sur les trois |
| 6.10 | UUID inexistant | `curl -s http://localhost:3000/api/v1/documents/00000000-0000-0000-0000-000000000000 -H "Authorization: Bearer $TOKEN"` | `404` |

## 7. API Tags (map clé→valeur) & Départements

Les tags sont une map `clé→valeur` portée par le document (plus de registre :
`GET`/`POST /api/v1/tags` n'existent plus). On les passe à l'upload avec
`tags[clé]=valeur` et on remplace la map entière via `PUT`.

| # | Cas | Commande | Attendu |
|---|---|---|---|
| 7.1 | Tags à l'upload | `curl -s -X POST http://localhost:3000/api/v1/documents -H "Authorization: Bearer $TOKEN" -F "department_id=1" -F "file=@/tmp/qa-doc.pdf" -F "tags[type]=Facture" -F "tags[urgent]="` | `201`, JSON `"tags":{"type":"Facture","urgent":null}` (clé en minuscules) |
| 7.2 | Remplacer la map d'un document | `curl -s -X PUT http://localhost:3000/api/v1/documents/<UUID>/tags -H "Authorization: Bearer $TOKEN" -d "tags[type]=devis" -d "tags[annee]=2026"` | `200`, `"tags":{"type":"devis","annee":"2026"}` (remplace tout, pas de fusion) |
| 7.3 | Vider la map | `curl -s -X PUT http://localhost:3000/api/v1/documents/<UUID>/tags -H "Authorization: Bearer $TOKEN"` | `200`, `"tags":{}` |
| 7.4 | Tags invalides (non-map ou valeur imbriquée) | `... -d "tags=flat"` | `422`, map inchangée |
| 7.5 | Anciens endpoints de registre supprimés | `curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/api/v1/tags -H "Authorization: Bearer $TOKEN"` | `404` |
| 7.6 | Lister les départements | `curl -s http://localhost:3000/api/v1/departments -H "Authorization: Bearer $TOKEN"` | Uniquement Finance + Invoices (pas HR) |
| 7.7 | Créer un sous-dossier via l'API | `curl -s -X POST http://localhost:3000/api/v1/departments -H "Authorization: Bearer $TOKEN" -d "name=2026" -d "parent_id=2"` | `201` |
| 7.8 | Créer un sous-dossier sous HR | idem `parent_id=3` | `403` |
| 7.9 | Créer un département **racine** via l'API | idem sans `parent_id` | `400` (réservé à l'admin) |

---

## 8. Upload asynchrone (callback)

L'upload async se déclenche en ajoutant `callback_url` à la requête : la GED
répond immédiatement `202` avec un `request_id`, stocke le fichier en tâche de
fond, puis POste `{request_id, status, document_id, error_message, tags}` sur
l'URL fournie (`tags` = la map du document créé, `null` en cas d'échec). On
peut aussi passer `tags[clé]=valeur` à l'upload async, comme en synchrone.

Option simple : créer une URL jetable sur https://webhook.site et l'utiliser
comme `callback_url`.

Option locale (vérifiée) : depuis les conteneurs, la machine hôte est
joignable via `host.docker.internal` :

```bash
# Terminal 1 — un récepteur local sur :9999 (ou webhook.site)
python3 -m http.server 9999   # les POST apparaissent dans les logs (405 mais visibles)

# Terminal 2
curl -s -w " [%{http_code}]" -X POST http://localhost:3000/api/v1/documents \
  -H "Authorization: Bearer $TOKEN" \
  -F "department_id=1" -F "file=@/tmp/qa-doc.pdf" \
  -F "callback_url=http://host.docker.internal:9999/cb"
```

| # | Cas | Attendu |
|---|---|---|
| 8.1 | Réponse immédiate | `202` `{"request_id":"<uuid>","status":"pending"}` |
| 8.2 | Callback reçu (quelques secondes après) | POST JSON `{"request_id":..., "status":"completed", "document_id":"<uuid>", "error_message":null, "tags":{...}}` |
| 8.3 | Le document existe | `GET /api/v1/documents/<document_id>/download` → contenu du fichier |
| 8.4 | Async vers un département interdit | `403` immédiat, aucun callback |
| 8.5 | `callback_url` injoignable | `202` quand même ; la livraison est retentée 5 fois puis abandonnée (visible dans `docker compose logs web`) |

---

## 9. Persistance

| # | Cas | Commande | Attendu |
|---|---|---|---|
| 9.1 | Redémarrage de la stack | `docker compose restart`, attendre `/up` → 200 | Les documents, départements, clients et comptes sont toujours là ; le download d'un document existant fonctionne |
| 9.2 | Recréation des conteneurs | `docker compose down && docker compose up -d` | Idem (les volumes survivent) |

---

## 10. Remise à zéro de l'environnement

Pour repartir d'un environnement vierge (⚠️ détruit toutes les données —
documents, base, clients OAuth) :

```bash
docker compose down -v
docker compose up --build -d
docker compose exec web bin/rails db:seed
```

## Rappels / limites connues (hors périmètre v1)

- Pas de recherche plein-texte dans le contenu des fichiers (filtres sur nom,
  tag (clé ou paire clé/valeur), département uniquement).
- Pas de versionnage des documents, pas de sauvegardes automatiques.
- HTTP uniquement (usage LAN) — pas de TLS.
- Les droits d'un client sont lecture/écriture complets sur ses départements
  (pas de lecture seule).
- Token : durée de vie 1 h, pas de refresh token (en redemander un).
