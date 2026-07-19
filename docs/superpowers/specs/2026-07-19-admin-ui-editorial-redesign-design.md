# Admin UI — Refonte « Éditorial » (design)

**Date :** 2026-07-19
**Statut :** validé (brainstorming avec maquettes navigateur — style B « SaaS
éditorial », sombre D1 « Nuit indigo », layout L1 topbar conservé)

## Objectif

Refondre l'identité visuelle du back-office admin (aujourd'hui « Console »
teal) vers une direction **SaaS éditoriale** — accent indigo, titres serif,
pills, surfaces arrondies — et corriger au passage les **8 oublis de style**
relevés à l'audit. Le toggle clair/sombre est conservé ; le thème sombre
devient la variante **Nuit indigo** (fonds bleu-violet teintés dans la masse).

## Contraintes non négociables

- **Aucun libellé visible ne change** — `test/system/admin_ui_test.rb` et
  `test/controllers/admin/*` s'accrochent aux labels/field names ; la refonte
  doit passer `bin/ci` sans toucher aux assertions.
- **Pas de framework** : un seul `application.css` (tokens CSS custom
  properties, Propshaft, pas de build), `application.js` vanilla via importmap
  (pas de Stimulus/Turbo).
- Le mécanisme de thème existant ne change pas : inline script anti-flash dans
  les `<head>`, `data-theme` sur `<html>`, persistance localStorage,
  `#theme-toggle`.
- Layout **topbar** conservé (L1) — pas de sidebar ; structure des vues
  inchangée hors corrections listées.

## 1. Tokens

### Clair (`:root`)

| Token | Valeur | Note |
|---|---|---|
| `--bg` | `#eef0f6` | fond froid légèrement violacé |
| `--surface` | `#ffffff` | cartes, topbar |
| `--surface-2` | `#f4f5fa` | zébrures, inputs |
| `--surface-3` | `#ececf4` | chips clé |
| `--text` | `#1d2138` | encre bleutée |
| `--muted` | `#6d7285` | |
| `--faint` | `#9aa0b5` | |
| `--border` | `#e7e9f2` | |
| `--border-strong` | `#d6d9e8` | |
| `--accent` | `#5b5bd6` | indigo |
| `--accent-ink` | `#4a4ab8` | liens, texte accent |
| `--accent-weak` | `#ececfb` | fonds pills |
| `--accent-contrast` | `#ffffff` | |
| ok / warn / bad / info | `#1e7a52` / `#9a6a12` / `#c33b2c` / `#2565c9` | + fonds pastel recalés sur la palette (ok-bg `#e3f2ea`, warn-bg `#f9eeda`, bad-bg `#fbe7e3`, info-bg `#e6eefc`) |
| `--radius` | `10px` | cartes |
| `--radius-sm` | `8px` | boutons, inputs |
| `--radius-pill` | `999px` | nav, badges, tags (nouveau) |
| `--shadow` | `0 2px 12px rgba(30,30,60,.08), 0 1px 2px rgba(30,30,60,.05)` | plus douce et présente |
| `--font-head` | `Georgia, "Times New Roman", serif` | h1, titres de section, brand |
| `--font-body` | stack système sans-serif (inchangée) | |
| `--font-mono` | stack mono (inchangée) | ids, dates, tags |

### Sombre (`:root[data-theme="dark"]`) — « Nuit indigo »

| Token | Valeur |
|---|---|
| `--bg` | `#0f1020` |
| `--surface` | `#15172b` |
| `--surface-2` | `#191c33` |
| `--surface-3` | `#232748` |
| `--text` | `#eceef8` |
| `--muted` | `#8285a3` |
| `--faint` | `#6f7292` |
| `--border` | `#262a45` |
| `--border-strong` | `#2e325a` |
| `--accent` | `#7a7ae6` |
| `--accent-ink` | `#b3b3f2` |
| `--accent-weak` | `#2b2e55` |
| `--accent-contrast` | `#0f1020` |
| ok / warn / bad / info | `#34d399` / `#f5b544` / `#f2a08f` / `#5aa9ff` + fonds translucides teintés (ex. bad-bg `#452b33`) |
| `--shadow` | `none` |

Le halo radial de la page login (`.auth-wrap` sombre) passe du teal à
l'indigo.

## 2. Composants (restylage CSS pur — classes et markup inchangés)

- **Topbar** : liens nav en pills (`--radius-pill`) ; l'actif = fond
  `--accent-weak`, texte `--accent-ink`, plus de bordure basse. Brand « GED »
  en `--font-head`.
- **h1 / .section-title / .auth-card h2** : `--font-head` (serif), tailles
  légèrement augmentées (h1 ~24px).
- **Badges de statut** : pills, point coloré conservé.
- **Tag chips** : pills scindées `key · value` (clé sur `--surface-3`, valeur
  sur `--accent-weak`), toujours mono.
- **Boutons** : rayon `--radius-sm`, primaire indigo ; hover inchangés dans
  l'esprit.
- **Tables** : en-têtes en petites capitales sans-serif sur `--surface-2`,
  séparateurs `--border` adoucis, hover de ligne conservé.
- **Cartes / filtres / form-card / stat / dropzone / auth-card / queue
  banner** : mêmes structures, nouveaux tokens (rayons, ombre, fonds).

## 3. Corrections des 8 oublis (touchent CSS **et** vues)

1. **Flashes sur le layout `application`** — `layouts/application.html.erb`
   rend le bloc `.flashes` (comme le layout admin) ; un échec de login Devise
   affiche enfin son message. Markup wrappé pour que la carte login reste
   centrée (flash au-dessus de `.auth-card`, dans `.auth-wrap`).
2. **Confirmations de suppression réelles** — l'app n'a pas Turbo, les
   `data: { turbo_confirm: … }` sont inertes. Remplacés par
   `data: { confirm: … }` + handler vanilla dans `application.js` : au
   `click` sur `button[data-confirm]` (les `button_to`), `window.confirm`
   sinon `preventDefault()`. Concerne les Delete de Documents et Departments ;
   on l'ajoute aussi au Delete de Clients (même risque, même geste).
3. **`.field_with_errors` stylé** — `display: contents` sur le wrapper (ne
   casse pas le flex de `.field`), bordure `--bad` sur l'input/label enfant.
4. **`.muted` / `.strong` globaux** — promus utilitaires racine (mêmes règles
   qu'aujourd'hui sous `.tbl`, qui deviennent redondantes et sont retirées) ;
   corrige Status (health rows), Trash et confirm_deletion (`p.muted`).
5. **Page `confirm_deletion` refaite** — contenu en `form-card` (padding),
   `<select>` avec classe `select`, labels via `.field`, et nouveau bouton
   **`btn-danger-solid`** (fond `--bad`, texte blanc) pour « Trash department
   and its documents » ; « Move documents… » reste `btn-primary`, « Cancel »
   reste `btn-link`. Libellés inchangés.
6. **`<code>` stylé** — règle globale : mono, fond `--surface-3`, padding
   `1px 5px`, rayon 4px (hints des formulaires tags).
7. **Empty states** — lignes `.empty` ajoutées aux index Departments
   (« No departments yet. »), Clients (« No registered clients. ») et Upload
   requests (« No upload requests match. ») — mêmes patterns que Documents et
   Trash. (Nouveaux libellés = nouveaux états, aucune assertion existante ne
   casse.)
8. **CSS mort supprimé** — `.check-grid` (le formulaire client utilise une
   table Read/Write depuis).

## 4. Hors périmètre

- Pas de sidebar, pas de changement de structure des vues au-delà des
  corrections ci-dessus, pas de nouveau JS au-delà du handler `data-confirm`,
  pas de webfonts (stacks système uniquement), pas de changement des layouts
  mailer.

## 5. Vérification

- `bin/ci` vert (tests + system tests + rubocop) **sans modifier les
  assertions existantes** ; seuls ajouts de tests autorisés (ex. présence du
  flash sur login raté si un test contrôleur s'y prête).
- Revue visuelle des deux thèmes sur `bin/dev` : login (+ échec), Documents
  (index/new avec dropzone/edit tags), Departments (+ confirm_deletion des
  deux branches), Clients (index/form grille R/W), Upload requests (banner
  pause/resume), Trash, Status.
- Contraste : viser AA sur texte normal dans les deux thèmes (les valeurs
  tokens ci-dessus ont été choisies pour ; vérifier au moins accent-ink sur
  surface et muted sur bg).
