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
