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
