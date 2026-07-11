/*
 * CAVS landing — live per-product version wiring.
 *
 * The three products release on independent tag trains:
 *   - core + SDKs   → tags `vX.Y.Z`            (crates.io / npm / maven / go)
 *   - engine plugins→ tags `plugins-vX.Y.Z`    (GitHub Release asset)
 *   - desktop app   → tags `desktop-vX.Y.Z`    (GitHub Release installers)
 *
 * This script fetches the repo's releases once, resolves the latest version of
 * each product, and drives the DOM by data-attributes so no version is ever
 * hardcoded in the HTML:
 *
 *   data-cavs-ver="core|plugins|desktop"    → textContent := "vX.Y.Z"
 *   data-cavs-link="core|plugins|desktop"   → href := that release's page
 *   data-cavs-vertext="core"                → textContent := bare version "X.Y.Z"
 *   data-cavs-dl="desktop-windows|desktop-macos|desktop-linux|plugins-godot"
 *                                           → href := direct asset download
 *
 * Registry buttons (cargo/npm/maven/go) are plain <a> links to the registry's
 * own "latest" page — the registry itself is the source of truth there, so no
 * JS is needed for them.
 */
(function () {
  "use strict";

  var REPO = "orelvis15/cavs";
  var API = "https://api.github.com/repos/" + REPO + "/releases?per_page=100";
  var RELEASES_URL = "https://github.com/" + REPO + "/releases";
  var CACHE_KEY = "cavs:releases:v1";
  var CACHE_TTL = 30 * 60 * 1000; // 30 min

  // How to recognize and present each product from its release tag.
  var PRODUCTS = {
    core: {
      match: function (t) { return /^v\d/.test(t); },
      fallbackLink: RELEASES_URL,
    },
    plugins: {
      match: function (t) { return /^plugins-v/.test(t); },
      fallbackLink: RELEASES_URL + "?q=plugins",
    },
    desktop: {
      match: function (t) { return /^desktop-v/.test(t); },
      fallbackLink: RELEASES_URL + "?q=desktop",
    },
  };

  // Match a release asset to a download slot by filename.
  var ASSET_MATCHERS = {
    "desktop-windows": function (n) { return /\.msi$/i.test(n) || /-setup\.exe$/i.test(n); },
    "desktop-macos": function (n) { return /\.dmg$/i.test(n); },
    "desktop-linux": function (n) { return /\.appimage$/i.test(n) || /\.deb$/i.test(n); },
    "plugins-godot": function (n) { return /godot.*\.zip$/i.test(n); },
    "plugins-unity": function (n) { return /unity.*\.zip$/i.test(n); },
    "plugins-unreal": function (n) { return /unreal.*\.zip$/i.test(n); },
  };

  function verText(tag) {
    // Strip a product prefix ("desktop-", "plugins-") but keep the leading v.
    var m = /(?:^|-)(v\d[\w.\-]*)$/.exec(tag);
    return m ? m[1] : tag;
  }

  function pickLatest(releases, product) {
    // releases arrive newest-first from the API; take the first non-draft,
    // non-prerelease match.
    for (var i = 0; i < releases.length; i++) {
      var r = releases[i];
      if (r.draft || r.prerelease) continue;
      if (PRODUCTS[product].match(r.tag_name)) return r;
    }
    return null;
  }

  function assetUrl(release, slot) {
    if (!release || !release.assets) return null;
    var match = ASSET_MATCHERS[slot];
    for (var i = 0; i < release.assets.length; i++) {
      var a = release.assets[i];
      if (match(a.name)) return a.browser_download_url;
    }
    return null;
  }

  function apply(releases) {
    var latest = {};
    Object.keys(PRODUCTS).forEach(function (p) { latest[p] = pickLatest(releases, p); });

    // Version text.
    document.querySelectorAll("[data-cavs-ver]").forEach(function (el) {
      var p = el.getAttribute("data-cavs-ver");
      var r = latest[p];
      el.textContent = r ? verText(r.tag_name) : "latest";
    });

    // Release page links.
    document.querySelectorAll("[data-cavs-link]").forEach(function (el) {
      var p = el.getAttribute("data-cavs-link");
      var r = latest[p];
      el.setAttribute("href", r ? r.html_url : PRODUCTS[p].fallbackLink);
    });

    // Bare version number inside SDK doc snippets (wraps only the number).
    document.querySelectorAll("[data-cavs-vertext]").forEach(function (el) {
      var p = el.getAttribute("data-cavs-vertext");
      var r = latest[p];
      if (r) el.textContent = verText(r.tag_name).replace(/^v/, "");
    });

    // Direct asset download links.
    document.querySelectorAll("[data-cavs-dl]").forEach(function (el) {
      var slot = el.getAttribute("data-cavs-dl");
      var product = slot.indexOf("desktop") === 0 ? "desktop" : "plugins";
      var r = latest[product];
      var url = assetUrl(r, slot);
      el.setAttribute("href", url || (r ? r.html_url : PRODUCTS[product].fallbackLink));
    });
  }

  function fromCache() {
    try {
      var raw = sessionStorage.getItem(CACHE_KEY);
      if (!raw) return null;
      var obj = JSON.parse(raw);
      if (!obj || (Date.now() - obj.t) > CACHE_TTL) return null;
      return obj.d;
    } catch (e) { return null; }
  }

  function toCache(data) {
    try { sessionStorage.setItem(CACHE_KEY, JSON.stringify({ t: Date.now(), d: data })); }
    catch (e) { /* private mode / quota — ignore */ }
  }

  function run() {
    var cached = fromCache();
    if (cached) { apply(cached); return; }
    fetch(API, { headers: { Accept: "application/vnd.github+json" } })
      .then(function (r) { return r.ok ? r.json() : []; })
      .then(function (data) {
        if (!Array.isArray(data)) data = [];
        toCache(data);
        apply(data);
      })
      .catch(function () { apply([]); });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", run);
  } else {
    run();
  }
})();
