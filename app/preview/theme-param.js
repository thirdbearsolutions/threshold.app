// ?theme=system|light|dark|threshold saves that choice before the boot
// script reads it; ?theme=first clears it, to see the first-open picker.
(function () {
  var t = new URLSearchParams(location.search).get("theme");
  if (t === "first") localStorage.removeItem("threshold.theme");
  else if (t) localStorage.setItem("threshold.theme", t);
})();
