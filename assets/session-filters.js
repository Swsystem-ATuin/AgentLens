// Bind through a same-origin script: the security policy blocks inline handlers.
(() => {
  "use strict";
  const form = document.querySelector("form[data-session-filters]");
  if (!form) return;
  form.addEventListener("change", (event) => {
    if (event.target.matches("select")) form.requestSubmit();
  });
})();
