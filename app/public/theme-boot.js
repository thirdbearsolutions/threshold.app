// Applies the saved theme before the first paint, so the window never
// opens in the wrong colours. A plain script, not a module: modules run
// after the page has painted once. src/theme.ts takes over from here.
(function () {
  var choice;
  try { choice = localStorage.getItem("threshold.theme"); } catch (e) {}
  var dark = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  var theme = choice === "light" || choice === "dark" || choice === "threshold" ? choice : dark ? "dark" : "light";
  document.documentElement.dataset.theme = theme;
})();
