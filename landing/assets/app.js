/* CAVS site — shared behaviour. No dependencies. */
(function () {
  "use strict";

  /* ---- theme toggle (default = system; explicit choice persisted) ---- */
  var root = document.documentElement;
  function metaTheme() {
    var m = document.querySelector('meta[name="theme-color"]');
    if (!m) { m = document.createElement("meta"); m.name = "theme-color"; document.head.appendChild(m); }
    return m;
  }
  function currentIsLight() {
    var t = root.getAttribute("data-theme");
    if (t) return t === "light";
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches;
  }
  function syncMeta() {
    metaTheme().content = currentIsLight() ? "#F6F8FC" : "#0A0E1A";
  }
  syncMeta();
  var toggle = document.getElementById("themeToggle");
  if (toggle) {
    toggle.addEventListener("click", function () {
      var next = currentIsLight() ? "dark" : "light";
      root.setAttribute("data-theme", next);
      try { localStorage.setItem("cavs-theme", next); } catch (e) {}
      syncMeta();
    });
  }
  // keep in sync with the OS when the user hasn't chosen explicitly
  if (window.matchMedia) {
    var mq = window.matchMedia("(prefers-color-scheme: light)");
    var onOS = function () { if (!root.getAttribute("data-theme")) syncMeta(); };
    if (mq.addEventListener) mq.addEventListener("change", onOS);
    else if (mq.addListener) mq.addListener(onOS);
  }

  /* ---- nav shadow on scroll ---- */
  var nav = document.getElementById("nav");
  if (nav) {
    var onScroll = function () { nav.classList.toggle("scrolled", window.scrollY > 20); };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  /* ---- mobile menu ---- */
  var burger = document.getElementById("burger");
  var navLinks = document.getElementById("navLinks");
  if (burger && navLinks) {
    burger.addEventListener("click", function () { navLinks.classList.toggle("open"); });
    navLinks.querySelectorAll("a").forEach(function (a) {
      a.addEventListener("click", function () { navLinks.classList.remove("open"); });
    });
  }

  /* ---- FAQ accordion ---- */
  document.querySelectorAll(".faq-q").forEach(function (q) {
    q.addEventListener("click", function () {
      var item = q.parentElement;
      var wasOpen = item.classList.contains("open");
      document.querySelectorAll(".faq-item").forEach(function (i) { i.classList.remove("open"); });
      if (!wasOpen) item.classList.add("open");
    });
  });

  /* ---- copy buttons (data-copy holds the text) ---- */
  document.querySelectorAll("[data-copy]").forEach(function (btn) {
    btn.addEventListener("click", function () {
      var text = btn.getAttribute("data-copy");
      navigator.clipboard.writeText(text).then(function () {
        var prev = btn.textContent;
        btn.textContent = "copied ✓";
        setTimeout(function () { btn.textContent = prev; }, 1600);
      });
    });
  });

  /* ---- scroll reveal ---- */
  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
      });
    }, { threshold: 0.12 });
    document.querySelectorAll(".reveal").forEach(function (el) { io.observe(el); });
  } else {
    document.querySelectorAll(".reveal").forEach(function (el) { el.classList.add("in"); });
  }

  /* ---- version badge: keep it in sync with the latest GitHub release ----
     The badge is hardcoded in the HTML so it is correct without JS and never
     flashes; this only upgrades it if GitHub reports a newer tag, so a future
     release shows up on the site with no code change. Failures are silent —
     the hardcoded value stands. */
  (function () {
    var badge = document.querySelector(".brand .tag");
    if (!badge || !window.fetch) return;
    fetch("https://api.github.com/repos/orelvis15/cavs/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (data) {
        var tag = data && data.tag_name;
        if (tag && /^v?\d+\.\d+/.test(tag) && badge.textContent.trim() !== tag) {
          badge.textContent = tag;
        }
      })
      .catch(function () {});
  })();
})();
