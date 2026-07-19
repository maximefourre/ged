# Admin UI « Éditorial » Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rethemer le back-office admin en direction « SaaS éditorial » (accent indigo, titres serif, pills, sombre « Nuit indigo ») et corriger les 8 oublis de style relevés à l'audit.

**Architecture:** Un seul fichier `application.css` (tokens CSS custom properties, Propshaft, pas de build) est réécrit avec les nouveaux tokens et composants ; les corrections comportementales (flash login, confirmations de suppression, page confirm_deletion, empty states) touchent 6 vues, le layout `application`, et `application.js` (vanilla, importmap). Spec source de vérité : `docs/superpowers/specs/2026-07-19-admin-ui-editorial-redesign-design.md`.

**Tech Stack:** Rails (ERB views), Propshaft CSS, importmap vanilla JS, Minitest + Capybara (headless Chrome).

## Global Constraints

- **Aucun libellé visible existant ne change** — les tests contrôleurs/système s'y accrochent. Les seules modifications autorisées dans les tests existants sont les wraps `accept_confirm { … }` de la Task 4 (interaction, pas assertion) ; les ajouts de nouveaux tests sont libres.
- Pas de framework CSS/JS, pas de webfonts : stacks système uniquement.
- Le mécanisme de thème (`data-theme` sur `<html>`, inline script anti-flash, `#theme-toggle`, localStorage) ne change pas.
- Layout topbar conservé ; pas de sidebar.
- Chaque task se termine par une suite de tests verte et un commit sur la branche du worktree. Jamais de commit sur `main`, jamais de merge sans validation explicite de l'utilisateur.
- Commits au format du repo : `style(admin): …`, `fix(admin): …`, `docs(…): …`.

---

### Task 1: Worktree, branche et baseline

**Files:**
- Create: worktree `../ged-editorial` sur branche `feature/admin-editorial-redesign`
- Create (commit): `docs/superpowers/specs/2026-07-19-admin-ui-editorial-redesign-design.md`, `docs/superpowers/plans/2026-07-19-admin-ui-editorial-redesign.md`

**Interfaces:**
- Produces: worktree opérationnel avec Postgres dédié (`bin/worktree-stack db`), spec + plan committés sur la branche, baseline de tests verte.

- [x] **Step 1 : Créer le worktree et la branche**

```bash
cd /Users/fourreto/Projects/vibecode/ged
git worktree add ../ged-editorial -b feature/admin-editorial-redesign
```

- [x] **Step 2 : Copier la spec et le plan (non committés dans l'arbre principal) dans le worktree, puis les committer**

```bash
cp docs/superpowers/specs/2026-07-19-admin-ui-editorial-redesign-design.md ../ged-editorial/docs/superpowers/specs/
cp docs/superpowers/plans/2026-07-19-admin-ui-editorial-redesign.md ../ged-editorial/docs/superpowers/plans/
cd ../ged-editorial
git add docs/superpowers/specs/2026-07-19-admin-ui-editorial-redesign-design.md docs/superpowers/plans/2026-07-19-admin-ui-editorial-redesign.md
git commit -m "docs(specs): admin UI editorial redesign — spec + plan"
```

- [x] **Step 3 : Démarrer le Postgres du worktree et vérifier la baseline**

```bash
bin/worktree-stack db
bin/rails test
```

Expected: `0 failures, 0 errors` (baseline verte avant toute modification).

---

### Task 2: Retheme CSS complet (tokens + composants + nouveaux utilitaires)

**Files:**
- Modify: `app/assets/stylesheets/application.css` (remplacement complet du fichier)

**Interfaces:**
- Produces: classes CSS nouvelles consommées par les tasks suivantes — `.flashes-floating` (Task 3), `.btn-danger-solid` (Task 5), `.muted`/`.strong` globaux, `code`, `.field_with_errors` (display: contents). `.check-grid` supprimé (mort). Toutes les classes existantes utilisées par les vues conservent leur nom.

- [ ] **Step 1 : Remplacer intégralement `app/assets/stylesheets/application.css` par :**

```css
/*
 * GED admin — "Éditorial" design system (Propshaft, plain CSS, no build).
 * Top bar navigation à pills, accent indigo, titres serif, identifiants mono.
 * Theme-aware : clair par défaut, sombre « Nuit indigo » via
 * :root[data-theme="dark"] (inline init script + toggle, localStorage).
 */

:root {
  --bg: #eef0f6;
  --surface: #ffffff;
  --surface-2: #f4f5fa;
  --surface-3: #ececf4;
  --text: #1d2138;
  --muted: #6d7285;
  --faint: #9aa0b5;
  --border: #e7e9f2;
  --border-strong: #d6d9e8;

  --accent: #5b5bd6;
  --accent-ink: #4a4ab8;
  --accent-weak: #ececfb;
  --accent-contrast: #ffffff;

  --ok: #1e7a52;  --ok-bg: #e3f2ea;
  --warn: #9a6a12; --warn-bg: #f9eeda;
  --bad: #c33b2c;  --bad-bg: #fbe7e3;
  --info: #2565c9; --info-bg: #e6eefc;

  --radius: 10px;
  --radius-sm: 8px;
  --radius-pill: 999px;
  --shadow: 0 2px 12px rgba(30, 30, 60, .08), 0 1px 2px rgba(30, 30, 60, .05);

  --font-head: Georgia, "Times New Roman", serif;
  --font-body: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  --font-mono: ui-monospace, "SF Mono", "JetBrains Mono", Menlo, Consolas, monospace;
}

:root[data-theme="dark"] {
  --bg: #0f1020;
  --surface: #15172b;
  --surface-2: #191c33;
  --surface-3: #232748;
  --text: #eceef8;
  --muted: #8285a3;
  --faint: #6f7292;
  --border: #262a45;
  --border-strong: #2e325a;

  --accent: #7a7ae6;
  --accent-ink: #b3b3f2;
  --accent-weak: #2b2e55;
  --accent-contrast: #0f1020;

  --ok: #34d399;  --ok-bg: #15332c;
  --warn: #f5b544; --warn-bg: #3b3122;
  --bad: #f2a08f;  --bad-bg: #452b33;
  --info: #5aa9ff; --info-bg: #1d2c4a;

  --shadow: none;
}

* { box-sizing: border-box; }

html { -webkit-text-size-adjust: 100%; }

body {
  margin: 0;
  font-family: var(--font-body);
  font-size: 14.5px;
  line-height: 1.5;
  color: var(--text);
  background: var(--bg);
  -webkit-font-smoothing: antialiased;
}

a { color: var(--accent-ink); }

/* Utilitaires globaux (utilisés hors tables : Status, Trash, confirm_deletion) */
.strong { font-weight: 600; color: var(--text); }
.muted { color: var(--muted); }

code {
  font-family: var(--font-mono);
  font-size: .92em;
  background: var(--surface-3);
  border-radius: 4px;
  padding: 1px 5px;
}

/* ============================ App shell (top bar) ============================ */
body.admin { display: flex; flex-direction: column; min-height: 100vh; }

.topbar {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 56px;
  padding: 0 clamp(12px, 2.5vw, 22px);
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 0;
  z-index: 10;
}

.brand {
  display: flex;
  align-items: baseline;
  gap: 8px;
  padding-right: 20px;
  margin-right: 6px;
  text-decoration: none;
  white-space: nowrap;
}
.brand b {
  font-family: var(--font-head);
  font-weight: 700;
  font-size: 19px;
  letter-spacing: .01em;
  color: var(--text);
}
.brand span {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: .16em;
  color: var(--accent);
  font-weight: 700;
}

.nav { display: flex; align-items: center; gap: 4px; min-width: 0; overflow-x: auto; }
.nav-link {
  display: flex;
  align-items: center;
  padding: 7px 13px;
  border-radius: var(--radius-pill);
  font-size: 13.5px;
  font-weight: 500;
  color: var(--muted);
  text-decoration: none;
  white-space: nowrap;
  transition: color .15s, background .15s;
}
.nav-link:hover { color: var(--text); background: var(--surface-2); }
.nav-link.is-active { color: var(--accent-ink); background: var(--accent-weak); font-weight: 600; }

.topbar-right { margin-left: auto; display: flex; align-items: center; gap: 10px; padding-left: 12px; }
.who { color: var(--muted); font-size: 13px; display: flex; align-items: center; gap: 8px; }
.avatar {
  width: 25px; height: 25px; border-radius: 50%; flex: none;
  background: var(--accent); color: var(--accent-contrast);
  display: grid; place-items: center; font-weight: 700; font-size: 12px;
}

.icon-btn {
  font: inherit;
  width: 34px; height: 34px;
  display: grid; place-items: center;
  cursor: pointer;
  border: 1px solid var(--border-strong);
  background: var(--surface);
  color: var(--text);
  border-radius: var(--radius-sm);
  font-size: 15px;
  line-height: 1;
  transition: background .15s;
}
.icon-btn:hover { background: var(--surface-2); }
.theme-ico-light { display: none; }
.theme-ico-dark { display: inline; }
:root[data-theme="dark"] .theme-ico-light { display: inline; }
:root[data-theme="dark"] .theme-ico-dark { display: none; }

.content { width: 100%; max-width: 1280px; margin: 0 auto; padding: 24px clamp(16px, 3vw, 34px) 46px; }

/* ============================ Flash ============================ */
.flashes { display: flex; flex-direction: column; gap: 8px; margin-bottom: 18px; }
.flash {
  padding: 11px 14px;
  border-radius: var(--radius-sm);
  font-size: 14px;
  font-weight: 500;
}
.flash-notice { background: var(--ok-bg); color: var(--ok); }
.flash-alert { background: var(--bad-bg); color: var(--bad); }

/* Variante flottante pour le layout hors-admin (page de login) : ne décale
   pas la carte centrée en plein écran. */
.flashes-floating {
  position: fixed;
  top: 16px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 50;
  width: min(92vw, 420px);
  margin: 0;
}
.flashes-floating .flash { border: 1px solid var(--border); box-shadow: var(--shadow); }

/* ============================ Page header ============================ */
.page-head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 16px;
  flex-wrap: wrap;
  margin: 0 0 18px;
}
.content h1 {
  font-family: var(--font-head);
  font-size: 24px;
  font-weight: 700;
  letter-spacing: 0;
  margin: 0;
  text-wrap: balance;
}
.page-sub { margin: 5px 0 0; color: var(--muted); font-size: 13px; }

/* ============================ Buttons ============================ */
.btn {
  font: inherit;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  white-space: nowrap;
  padding: 8px 14px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-strong);
  background: var(--surface);
  color: var(--text);
  text-decoration: none;
  transition: filter .15s, background .15s, border-color .15s;
}
.btn:hover { background: var(--surface-2); }
.btn-primary { background: var(--accent); border-color: var(--accent); color: var(--accent-contrast); }
.btn-primary:hover { filter: brightness(1.06); background: var(--accent); }
.btn-sm { padding: 6px 10px; font-size: 12.5px; }
.btn-link { background: transparent; border-color: transparent; color: var(--accent-ink); padding: 6px 8px; }
.btn-link:hover { background: var(--accent-weak); }
.btn-danger { background: transparent; border-color: transparent; color: var(--bad); padding: 6px 8px; }
.btn-danger:hover { background: var(--bad-bg); }
.btn-danger-solid { background: var(--bad); border-color: var(--bad); color: var(--accent-contrast); }
.btn-danger-solid:hover { filter: brightness(1.06); background: var(--bad); }

/* button_to renders a <form>; keep those inline within action groups */
.row-actions form, .queue form, .page-head form.inline { display: inline; margin: 0; }

/* ============================ Filters ============================ */
.filters {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  align-items: center;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 12px;
  margin-bottom: 16px;
  box-shadow: var(--shadow);
}

/* ============================ Inputs ============================ */
.input, .select, textarea.input {
  font: inherit;
  font-size: 13.5px;
  color: var(--text);
  background: var(--surface-2);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-sm);
  padding: 8px 11px;
  min-width: 0;
}
.input::placeholder { color: var(--faint); }
.input.grow { flex: 1 1 200px; }
textarea.input { width: 100%; resize: vertical; font-family: var(--font-mono); font-size: 13px; }

.input:focus, .select:focus, .btn:focus-visible, .nav-link:focus-visible,
.icon-btn:focus-visible, .dropzone:focus-within, a:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

/* Validation re-render : Rails enveloppe champs et labels fautifs dans
   <div class="field_with_errors">. display:contents neutralise le wrapper
   dans le flex des .field ; l'état d'erreur se voit sur le champ lui-même. */
.field_with_errors { display: contents; }
.field_with_errors .input, .field_with_errors .select { border-color: var(--bad); }
.field_with_errors label { color: var(--bad); }

/* ============================ Cards & tables ============================ */
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
  box-shadow: var(--shadow);
}
.table-wrap { overflow-x: auto; }
table.tbl { width: 100%; border-collapse: collapse; font-size: 13px; }
.tbl th {
  text-align: left;
  font-weight: 600;
  color: var(--muted);
  font-size: 11px;
  letter-spacing: .06em;
  text-transform: uppercase;
  padding: 10px 14px;
  background: var(--surface-2);
  border-bottom: 1px solid var(--border);
  white-space: nowrap;
}
.tbl td { padding: 9px 14px; border-bottom: 1px solid var(--border); vertical-align: middle; }
.tbl tbody tr:last-child td { border-bottom: 0; }
.tbl tbody tr:hover td { background: var(--surface-2); }
.tbl .id { font-family: var(--font-mono); font-size: 12px; color: var(--muted); }
.tbl .mono { font-family: var(--font-mono); }
.tbl .date { color: var(--muted); white-space: nowrap; font-family: var(--font-mono); font-size: 12px; }
.tbl tr.trashed td { color: var(--muted); }
.tbl tr.trashed td.strong { color: var(--muted); font-weight: 600; }
.row-actions { display: flex; gap: 2px; justify-content: flex-end; align-items: center; white-space: nowrap; }
.dash { color: var(--faint); }

/* Empty state */
.empty { padding: 34px 16px; text-align: center; color: var(--muted); }

/* ==================== Readonly metadata panel ===================== */
.meta-card { padding: 15px 18px; margin-bottom: 16px; }
.meta-list {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: 14px 22px;
  margin: 0;
}
.meta-list dt {
  color: var(--muted);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: .06em;
  text-transform: uppercase;
  margin-bottom: 3px;
}
.meta-list dd { margin: 0; color: var(--text); font-size: 13.5px; }
.meta-list .mono { font-family: var(--font-mono); font-size: 12.5px; }

/* ============================ Tag chips ============================ */
.tags { display: flex; flex-wrap: wrap; gap: 5px; }
.tag {
  display: inline-flex;
  align-items: center;
  font-family: var(--font-mono);
  font-size: 11.5px;
  line-height: 1;
  border-radius: var(--radius-pill);
  overflow: hidden;
  border: 1px solid var(--border-strong);
}
.tag .k { padding: 4px 8px; background: var(--surface-3); color: var(--muted); }
.tag .v { padding: 4px 9px; background: var(--accent-weak); color: var(--accent-ink); font-weight: 600; }
.tag.solo .k { background: var(--accent-weak); color: var(--accent-ink); font-weight: 600; }

/* ============================ Status badges ============================ */
.badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  padding: 4px 11px 4px 9px;
  border-radius: var(--radius-pill);
  background: var(--surface-3);
  color: var(--muted);
}
.badge::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
.badge.ok { color: var(--ok); background: var(--ok-bg); }
.badge.warn { color: var(--warn); background: var(--warn-bg); }
.badge.bad { color: var(--bad); background: var(--bad-bg); }
.badge.info { color: var(--info); background: var(--info-bg); }

/* ============================ Queue banner ============================ */
.queue {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  background: var(--surface);
  border: 1px solid var(--border);
  border-left: 3px solid var(--ok);
  border-radius: var(--radius);
  padding: 13px 16px;
  margin-bottom: 16px;
  box-shadow: var(--shadow);
}
.queue.is-paused { border-left-color: var(--warn); }
.queue .counts { color: var(--muted); font-size: 13px; }
.queue .counts b { color: var(--text); font-family: var(--font-mono); }
.queue .spacer { flex: 1; }

/* ============================ Pagination ============================ */
.pager { display: flex; gap: 8px; justify-content: flex-end; margin-top: 16px; }

/* ============================ Forms (new/edit) ============================ */
.form-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  padding: 22px;
  max-width: 720px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.form-card p { margin: 0; }
.field { display: flex; flex-direction: column; gap: 6px; }
.field > label, .field-label {
  font-size: 11.5px;
  letter-spacing: .08em;
  text-transform: uppercase;
  color: var(--muted);
  font-weight: 700;
}
.field .input, .field .select { width: 100%; }
.field .hint { color: var(--muted); font-size: 12.5px; font-weight: 400; text-transform: none; letter-spacing: 0; }
.form-actions { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; }
.form-actions form { margin: 0; }

/* ============================ Dropzone ============================ */
.dropzone {
  position: relative;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  text-align: center;
  padding: 34px 20px;
  border: 2px dashed var(--border-strong);
  border-radius: var(--radius);
  background: var(--surface-2);
  color: var(--muted);
  cursor: pointer;
  transition: border-color .15s, background .15s;
}
.dropzone:hover { border-color: var(--accent); }
.dropzone.is-drag { border-color: var(--accent); background: var(--accent-weak); }
/* The native file input stays visible (and interactable for tests); the whole
   zone additionally accepts drops via JS. Its button is themed via the
   ::file-selector-button pseudo-element. */
.dropzone input[type="file"] { margin-top: 10px; font: inherit; font-size: 13px; color: var(--muted); max-width: 100%; }
.dropzone input[type="file"]::file-selector-button {
  font: inherit;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  padding: 8px 14px;
  margin-right: 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-strong);
  background: var(--surface);
  color: var(--text);
  transition: background .15s, border-color .15s;
}
.dropzone input[type="file"]::file-selector-button:hover { background: var(--surface-2); border-color: var(--accent); }
.dropzone .dz-icon { font-size: 26px; line-height: 1; color: var(--accent); }
.dropzone .dz-primary { font-weight: 600; color: var(--text); }
.dropzone .dz-file { font-family: var(--font-mono); font-size: 13px; color: var(--accent-ink); }

/* ============================ Auth (login) ============================ */
.auth-wrap {
  min-height: 100vh;
  display: grid;
  place-items: center;
  padding: 24px;
  background: var(--bg);
}
:root[data-theme="dark"] .auth-wrap {
  background:
    radial-gradient(1100px 480px at 50% -10%, rgba(122, 122, 230, .12), transparent 70%), var(--bg);
}
.auth-card {
  width: 100%;
  max-width: 380px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: calc(var(--radius) + 3px);
  padding: 30px 28px 26px;
  box-shadow: var(--shadow);
}
.auth-brand { display: flex; align-items: baseline; gap: 8px; margin-bottom: 4px; }
.auth-brand b { font-family: var(--font-head); font-weight: 700; font-size: 19px; letter-spacing: .01em; color: var(--text); }
.auth-brand span { font-size: 11px; text-transform: uppercase; letter-spacing: .16em; color: var(--accent); font-weight: 700; }
.auth-card h2 { font-family: var(--font-head); font-size: 20px; font-weight: 700; margin: 16px 0 4px; }
.auth-lede { margin: 0 0 20px; color: var(--muted); font-size: 13.5px; }
.auth-card .field { margin-bottom: 14px; }
.auth-card .btn-primary { width: 100%; justify-content: center; margin-top: 6px; padding: 11px; }

/* ============================ Status page ============================ */
.section-title { font-family: var(--font-head); font-size: 15px; font-weight: 700; margin: 22px 0 10px; color: var(--text); }
.stat-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 10px; margin-bottom: 14px; }
.stat { background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 12px 14px; box-shadow: var(--shadow); }
.stat .num { display: block; font-size: 22px; font-weight: 700; font-variant-numeric: tabular-nums; }
.stat .lbl { font-size: 12px; color: var(--muted); }
.health-list { list-style: none; margin: 0; padding: 0; }
.health-row { display: flex; align-items: center; gap: 12px; padding: 10px 14px; border-bottom: 1px solid var(--border); }
.health-row:last-child { border-bottom: 0; }

/* ============================ Motion / responsive ============================ */
@media (prefers-reduced-motion: reduce) { * { transition: none !important; } }

@media (max-width: 720px) {
  .topbar { height: auto; flex-wrap: wrap; padding: 8px 14px; gap: 4px 8px; }
  .brand { padding-right: 8px; }
  .topbar-right { width: 100%; order: 3; justify-content: flex-end; padding: 0; }
  .nav { order: 4; width: 100%; }
}
```

Notes de relecture pour l'implémenteur : par rapport à l'ancien fichier, ce
remplacement (a) change les tokens des deux thèmes, (b) passe nav/badges/tags
en pills, (c) passe h1/brand/section-title/auth h2 en serif, (d) ajoute
`.strong`/`.muted` globaux, `code`, `.field_with_errors`, `.btn-danger-solid`,
`.flashes-floating`, `.form-card p`, `.form-actions form`, (e) supprime
`.tbl .strong`/`.tbl .muted` (couverts par les globaux — la cascade trashed
`.tbl tr.trashed td.strong` reste) et tout le bloc `.check-grid` (mort).

- [x] **Step 2 : Vérifier que rien ne casse**

```bash
bin/rails test
bin/rails test:system
```

Expected: `0 failures, 0 errors` aux deux (aucun libellé ni classe consommée par les vues n'a changé).

- [x] **Step 3 : Commit**

```bash
git add app/assets/stylesheets/application.css
git commit -m "style(admin): editorial retheme — indigo/serif/pills tokens, dark 'nuit indigo'"
```

---

### Task 3: Flash sur le layout `application` (feedback de login raté)

**Files:**
- Modify: `app/views/layouts/application.html.erb` (bloc `<body>`)
- Test: `test/controllers/admin/authentication_test.rb` (ajout d'un test)

**Interfaces:**
- Consumes: `.flashes`, `.flash-*`, `.flashes-floating` (Task 2).

- [x] **Step 1 : Écrire le test qui échoue** — ajouter à la fin de la classe dans `test/controllers/admin/authentication_test.rb` :

```ruby
  test "failed login re-renders the form with the devise alert visible" do
    Admin.create!(username: "alice", password: "secret123")
    post admin_session_path, params: { admin: { username: "alice", password: "wrong" } }
    assert_match "Invalid", response.body
  end
```

- [x] **Step 2 : Vérifier qu'il échoue**

```bash
bin/rails test test/controllers/admin/authentication_test.rb
```

Expected: FAIL — le layout `application` ne rend aucun flash, le corps ne contient pas le message Devise.

- [x] **Step 3 : Rendre les flashes dans le layout** — dans `app/views/layouts/application.html.erb`, remplacer :

```erb
  <body>
    <%= yield %>
  </body>
```

par :

```erb
  <body>
    <% if flash.any? %>
      <div class="flashes flashes-floating">
        <% flash.each do |kind, msg| %>
          <div class="flash flash-<%= kind %>"><%= msg %></div>
        <% end %>
      </div>
    <% end %>
    <%= yield %>
  </body>
```

- [x] **Step 4 : Vérifier que le test passe**

```bash
bin/rails test test/controllers/admin/authentication_test.rb
```

Expected: PASS.

- [x] **Step 5 : Commit**

```bash
git add app/views/layouts/application.html.erb test/controllers/admin/authentication_test.rb
git commit -m "fix(admin): show devise flash on the login layout (failed login had no feedback)"
```

---

### Task 4: Confirmations de suppression réelles (`data-confirm` vanilla)

**Files:**
- Modify: `app/javascript/application.js` (ajout du handler)
- Modify: `app/views/admin/documents/index.html.erb` (bouton Delete)
- Modify: `app/views/admin/departments/index.html.erb` (bouton Delete)
- Modify: `app/views/admin/app_clients/index.html.erb` (bouton Delete — gagne une confirmation)
- Test: `test/system/admin_ui_test.rb` (wraps `accept_confirm` — seuls changements autorisés dans un test existant)

**Interfaces:**
- Produces: attribut `data-confirm` sur les boutons de suppression ; handler global `click` dans `application.js`.

- [ ] **Step 1 : Ajouter le handler à la fin de `app/javascript/application.js`**

```js
// Delete confirmations (pas de Turbo dans cette app) : tout bouton portant
// data-confirm demande confirmation avant que son <form> (button_to) parte.
(function () {
  document.addEventListener("click", function (e) {
    var btn = e.target.closest("button[data-confirm]");
    if (btn && !window.confirm(btn.dataset.confirm)) e.preventDefault();
  });
})();
```

- [ ] **Step 2 : Basculer les vues de `turbo_confirm` (inerte) vers `confirm`**

Dans `app/views/admin/documents/index.html.erb` :

```erb
                <%= button_to "Delete", admin_document_path(doc), method: :delete,
                    class: "btn btn-danger btn-sm",
                    data: { confirm: "Delete #{doc.filename}?" } %>
```

Dans `app/views/admin/departments/index.html.erb` :

```erb
                  <%= button_to "Delete", admin_department_path(dept), method: :delete,
                      class: "btn btn-danger btn-sm",
                      data: { confirm: "Delete #{dept.name}?" } %>
```

Dans `app/views/admin/app_clients/index.html.erb` (ajout — ce bouton n'avait aucune confirmation) :

```erb
                <%= button_to "Delete", admin_app_client_path(client), method: :delete,
                    class: "btn btn-danger btn-sm",
                    data: { confirm: "Delete #{client.client_identifier}?" } %>
```

- [ ] **Step 3 : Adapter les interactions des tests système** — dans `test/system/admin_ui_test.rb` :

Test 2.9 : remplacer

```ruby
    # The admin layout ships no JavaScript, so the turbo-confirm never fires;
    # the delete button just submits its form.
    within("tr", text: "report.pdf") { click_button "Delete" }
```

par

```ruby
    # Le bouton Delete porte un data-confirm géré en vanilla JS.
    within("tr", text: "report.pdf") { accept_confirm { click_button "Delete" } }
```

Test 2.9b : remplacer

```ruby
    within("tr", text: "report.pdf") { click_button "Delete" }
```

par

```ruby
    within("tr", text: "report.pdf") { accept_confirm { click_button "Delete" } }
```

Test 2.10 : remplacer

```ruby
    within("tr", text: "Finance") { click_button "Delete" }
```

par

```ruby
    within("tr", text: "Finance") { accept_confirm { click_button "Delete" } }
```

(Aucune assertion ne change ; seuls ces trois clics sont wrappés.)

- [ ] **Step 4 : Lancer les tests système et contrôleurs**

```bash
bin/rails test:system
bin/rails test
```

Expected: `0 failures, 0 errors` aux deux. Les tests contrôleurs ne exécutent pas de JS et ne voient que le changement d'attribut `data`.

- [x] **Step 5 : Commit**

```bash
git add app/javascript/application.js app/views/admin/documents/index.html.erb app/views/admin/departments/index.html.erb app/views/admin/app_clients/index.html.erb test/system/admin_ui_test.rb
git commit -m "fix(admin): real delete confirmations via vanilla data-confirm (turbo_confirm was inert)"
```

---

### Task 5: Refonte de la page `confirm_deletion`

**Files:**
- Modify: `app/views/admin/departments/confirm_deletion.html.erb` (remplacement complet)

**Interfaces:**
- Consumes: `.form-card`, `.form-card p`, `.form-actions form`, `.select`, `.btn-danger-solid`, `.muted` global (Task 2).
- Libellés inchangés : « Move everything to », « Move documents and delete department », « Trash department and its documents », « Cancel » (les tests système 2.10 s'y accrochent).

- [x] **Step 1 : Remplacer intégralement `app/views/admin/departments/confirm_deletion.html.erb` par :**

```erb
<header class="page-head">
  <h1>Delete <%= @department.name %></h1>
</header>

<div class="form-card">
  <p>This department holds <%= @live_docs_count %> active and <%= @trashed_docs_count %> trashed document(s).</p>
  <% if @pending_requests_count.positive? %>
    <p class="muted"><%= @pending_requests_count %> pending upload request(s): trashing the department will fail them; moving re-targets them.</p>
  <% end %>

  <% if @target_departments.exists? %>
    <%= form_with url: move_documents_admin_department_path(@department), method: :post do %>
      <div class="field">
        <label for="target_department_id">Move everything to</label>
        <select name="target_department_id" id="target_department_id" class="select">
          <% @target_departments.each do |dept| %>
            <option value="<%= dept.id %>" <%= "selected" if dept.id == @default_target&.id %>><%= dept.name %></option>
          <% end %>
        </select>
      </div>
      <div class="form-actions">
        <%= submit_tag "Move documents and delete department", class: "btn btn-primary" %>
      </div>
    <% end %>
  <% else %>
    <p class="muted">No other active department to move documents into.</p>
  <% end %>

  <div class="form-actions">
    <%= button_to "Trash department and its documents", trash_admin_department_path(@department), method: :post, class: "btn btn-danger-solid" %>
    <%= link_to "Cancel", admin_departments_path, class: "btn btn-link" %>
  </div>
</div>
```

(Changements : `.card` nu → `.form-card` paddé ; le `<select>` gagne la classe `select` ; le `<div class="field">` espace le formulaire de move ; « Trash… » passe du style lien fantôme `btn-danger` au bouton plein `btn-danger-solid` ; le petit `<div class="field">` du move est suivi d'un `form-actions` pour l'alignement. Aucun texte ne change.)

- [x] **Step 2 : Vérifier les tests qui couvrent la page**

```bash
bin/rails test test/controllers/admin/departments_controller_test.rb
bin/rails test:system
```

Expected: `0 failures, 0 errors` (2.10 traverse la page et clique les deux boutons par libellé).

- [x] **Step 3 : Commit**

```bash
git add app/views/admin/departments/confirm_deletion.html.erb
git commit -m "style(admin): confirm_deletion page — form-card layout, styled select, solid danger action"
```

---

### Task 6: Empty states (Departments, Clients, Upload requests)

**Files:**
- Modify: `app/views/admin/departments/index.html.erb` (ligne empty)
- Modify: `app/views/admin/app_clients/index.html.erb` (ligne empty)
- Modify: `app/views/admin/upload_requests/index.html.erb` (ligne empty)
- Test: `test/controllers/admin/departments_controller_test.rb`, `test/controllers/admin/app_clients_controller_test.rb`, `test/controllers/admin/upload_requests_controller_test.rb` (un test ajouté chacun)

**Interfaces:**
- Consumes: `.empty` (existant). Pattern identique à Documents/Trash : `<tr><td colspan=N class="empty">…</td></tr>` quand la collection est vide.

- [ ] **Step 1 : Écrire les trois tests qui échouent**

Ajout dans `test/controllers/admin/departments_controller_test.rb` (le setup signe déjà l'admin) :

```ruby
  test "index shows an empty state when there are no departments" do
    get admin_departments_path
    assert_response :success
    assert_match "No departments yet.", response.body
  end
```

Ajout dans `test/controllers/admin/app_clients_controller_test.rb` (le setup ne signe pas : appeler `sign_in @admin` dans le test) :

```ruby
  test "index shows an empty state when there are no clients" do
    sign_in @admin
    get admin_app_clients_path
    assert_response :success
    assert_match "No registered clients.", response.body
  end
```

Ajout dans `test/controllers/admin/upload_requests_controller_test.rb` (idem, `sign_in @admin` dans le test) :

```ruby
  test "index shows an empty state when there are no upload requests" do
    sign_in @admin
    get admin_upload_requests_path
    assert_response :success
    assert_match "No upload requests match.", response.body
  end
```

- [x] **Step 2 : Vérifier qu'ils échouent**

```bash
bin/rails test test/controllers/admin/departments_controller_test.rb test/controllers/admin/app_clients_controller_test.rb test/controllers/admin/upload_requests_controller_test.rb
```

Expected: 3 FAIL (les vues ne rendent rien quand la collection est vide).

- [ ] **Step 3 : Ajouter les lignes empty aux trois vues**

`app/views/admin/departments/index.html.erb` — après la boucle `@departments.each`, avant `</tbody>` (table à 4 colonnes) :

```erb
        <% if @departments.empty? %>
          <tr><td colspan="4" class="empty">No departments yet.</td></tr>
        <% end %>
```

`app/views/admin/app_clients/index.html.erb` — après la boucle `@app_clients.each`, avant `</tbody>` (table à 4 colonnes) :

```erb
        <% if @app_clients.empty? %>
          <tr><td colspan="4" class="empty">No registered clients.</td></tr>
        <% end %>
```

`app/views/admin/upload_requests/index.html.erb` — après la boucle `@upload_requests.each`, avant `</tbody>` (table à 8 colonnes) :

```erb
        <% if @upload_requests.empty? %>
          <tr><td colspan="8" class="empty">No upload requests match.</td></tr>
        <% end %>
```

- [x] **Step 4 : Vérifier que les trois tests passent**

```bash
bin/rails test test/controllers/admin/departments_controller_test.rb test/controllers/admin/app_clients_controller_test.rb test/controllers/admin/upload_requests_controller_test.rb
```

Expected: PASS partout.

- [x] **Step 5 : Commit**

```bash
git add app/views/admin/departments/index.html.erb app/views/admin/app_clients/index.html.erb app/views/admin/upload_requests/index.html.erb test/controllers/admin/departments_controller_test.rb test/controllers/admin/app_clients_controller_test.rb test/controllers/admin/upload_requests_controller_test.rb
git commit -m "feat(admin): empty states on departments, clients and upload requests indexes"
```

---

### Task 7: Validation finale — `bin/ci`, revue visuelle, stack isolée

**Files:**
- Aucun nouveau fichier ; validation.

- [x] **Step 1 : Suite complète + lint**

```bash
bin/ci
```

Expected: tests, tests système et RuboCop tous verts. Rapporter la sortie réelle (jamais « ça passe » sans l'avoir vue).

- [x] **Step 2 : Rebase sur main à jour**

```bash
git fetch origin && git rebase main
bin/rails test
```

Expected: rebase propre, tests toujours verts.

- [x] **Step 3 : Monter la stack isolée du worktree pour la validation visuelle**

```bash
bin/worktree-stack up
```

Puis vérifier à la main, dans les DEUX thèmes (toggle en topbar) : login (+ un échec de login → flash flottant visible), Documents (index, new avec dropzone, edit tags avec hint `code`), Departments (+ delete d'un département plein → page confirm_deletion refaite, boutons Move/Trash/Cancel), Clients (index, grille R/W, delete avec confirm), Upload requests (banner pause/resume, empty state), Trash, Status (health rows `.strong`/`.muted` désormais stylés).

- [ ] **Step 4 : S'arrêter et demander la validation de l'utilisateur**

Ne PAS merger. Présenter le diff, l'URL de la stack du worktree, et attendre la validation explicite avant tout `git rebase main` + `merge --ff-only` (skill superpowers:finishing-a-development-branch).
