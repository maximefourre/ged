// GED admin — minimal vanilla enhancements (no Stimulus/Turbo in this app).

// Light/dark toggle. The initial theme is set inline in the layout <head> to
// avoid a flash; here we only handle the click and persist the choice.
(function () {
  var btn = document.getElementById("theme-toggle");
  if (!btn) return;
  btn.addEventListener("click", function () {
    var next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem("ged-theme", next); } catch (e) {}
  });
})();

// Drag-and-drop file upload (progressive enhancement). A file <input> covers the
// zone, so click-to-pick and native drop already work without JS; this adds the
// drag highlight and shows the chosen filename.
(function () {
  document.querySelectorAll("[data-dropzone]").forEach(function (zone) {
    var input = zone.querySelector('input[type="file"]');
    if (!input) return;
    var label = zone.querySelector("[data-dropzone-file]");

    function showName() {
      if (!label) return;
      label.textContent = input.files && input.files.length ? input.files[0].name : "";
    }
    input.addEventListener("change", showName);

    ["dragenter", "dragover"].forEach(function (ev) {
      zone.addEventListener(ev, function (e) { e.preventDefault(); zone.classList.add("is-drag"); });
    });
    ["dragleave", "dragend", "drop"].forEach(function (ev) {
      zone.addEventListener(ev, function (e) { e.preventDefault(); zone.classList.remove("is-drag"); });
    });
    zone.addEventListener("drop", function (e) {
      if (e.dataTransfer && e.dataTransfer.files.length) {
        input.files = e.dataTransfer.files;
        showName();
      }
    });
  });
})();

// Delete confirmations (pas de Turbo dans cette app) : tout bouton portant
// data-confirm demande confirmation avant que son <form> (button_to) parte.
(function () {
  document.addEventListener("click", function (e) {
    var btn = e.target.closest("button[data-confirm]");
    if (btn && !window.confirm(btn.dataset.confirm)) e.preventDefault();
  });
})();

// Collapsible department tree (admin/departments#index). Rows carry
// data-dept-id / data-parent-id; a [data-tree-toggle] button hides or shows
// its subtree. Everything starts expanded, no persistence across reloads.
(function () {
  function childRows(deptId) {
    return document.querySelectorAll('tr[data-parent-id="' + deptId + '"]');
  }

  // Collapsing hides every descendant row unconditionally. Expanding only
  // reveals direct children, then recurses into a child's own subtree only
  // if that child's toggle is still expanded — so a nested collapsed branch
  // stays collapsed when its ancestor re-opens.
  function setDescendantsVisible(deptId, visible) {
    childRows(deptId).forEach(function (row) {
      row.hidden = !visible;
      var childToggle = row.querySelector("[data-tree-toggle]");
      if (!visible) {
        setDescendantsVisible(row.dataset.deptId, false);
      } else if (!childToggle || childToggle.getAttribute("aria-expanded") === "true") {
        setDescendantsVisible(row.dataset.deptId, true);
      }
    });
  }

  document.addEventListener("click", function (e) {
    var btn = e.target.closest("[data-tree-toggle]");
    if (!btn) return;
    var row = btn.closest("tr");
    if (!row) return;

    var expanded = btn.getAttribute("aria-expanded") === "true";
    var next = !expanded;
    btn.setAttribute("aria-expanded", String(next));
    btn.textContent = next ? "▾" : "▸";
    setDescendantsVisible(row.dataset.deptId, next);
  });
})();

// Split-pane document preview (admin/documents#index). Clicking a row loads
// an inline preview on the right via an iframe (sandboxed, no scripts/same
// origin) for previewable types; other types get a fallback with a Download
// link. Clicks on links/buttons inside the row (Download/Tags/Delete) are
// left alone.
(function () {
  var panel = document.querySelector("[data-doc-preview]");
  var split = document.querySelector(".doc-split");
  if (!panel || !split) return;

  var body = panel.querySelector("[data-preview-body]");
  var nameEl = panel.querySelector("[data-preview-name]");
  var downloadLink = panel.querySelector("[data-preview-download]");
  var closeBtn = panel.querySelector("[data-preview-close]");
  var activeRow = null;

  function clearActive() {
    if (activeRow) activeRow.classList.remove("is-active");
    activeRow = null;
  }

  function closePreview() {
    panel.hidden = true;
    split.classList.remove("is-open");
    body.innerHTML = "";
    clearActive();
  }

  document.addEventListener("click", function (e) {
    if (e.target.closest("a, button")) return;

    var row = e.target.closest("[data-doc-row]");
    if (!row) return;

    var previewUrl = row.dataset.previewUrl;
    var contentType = row.dataset.contentType;
    var filename = row.dataset.filename;
    var downloadUrl = row.dataset.downloadUrl;

    clearActive();
    row.classList.add("is-active");
    activeRow = row;

    panel.hidden = false;
    split.classList.add("is-open");

    nameEl.textContent = filename;
    downloadLink.href = downloadUrl;

    var type = contentType || "";
    var ok = type === "application/pdf" || /^image\//.test(type) || /^text\//.test(type);

    body.innerHTML = "";
    if (ok) {
      var f = document.createElement("iframe");
      f.className = "doc-preview-frame";
      f.setAttribute("sandbox", "");
      f.src = previewUrl;
      body.appendChild(f);
    } else {
      var empty = document.createElement("div");
      empty.className = "doc-preview-empty";
      empty.innerHTML = "<p>Aucun aperçu disponible pour ce type</p>" +
        '<a class="btn btn-sm" href="' + downloadUrl + '">Download</a>';
      body.appendChild(empty);
    }
  });

  closeBtn.addEventListener("click", closePreview);
})();
