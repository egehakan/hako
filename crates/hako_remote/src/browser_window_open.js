// Runs in the page's own world on pages of this computer. A pop-up window would open on the Mac's
// screen and take the focus from its owner; without its size and position features it opens as
// a tab of the same off-screen window instead, still with its opener, which sign-in pop-ups
// answer through.
(() => {
  const host = location.hostname.replace(/\.$/, "").toLowerCase();
  const local =
    host === "localhost" ||
    host.endsWith(".localhost") ||
    /^127(\.\d{1,3}){3}$/.test(host) ||
    host === "[::1]" ||
    host === "0.0.0.0";
  if (!local) return;
  const open = window.open;
  const kept = /^\s*(noopener|noreferrer)\s*(=|$)/i;
  window.open = function (url, target, features) {
    if (typeof features === "string") {
      features = features
        .split(",")
        .filter((feature) => kept.test(feature))
        .join(",");
    }
    return open.call(this, url, target, features);
  };
})();
