/* ==========================================================================
   NCLR — page routing, 3D page-turn transitions and interface behaviour
   ========================================================================== */
(function () {
  "use strict";

  var ROUTES = [
    { id: "home", label: "主页", title: "NCLR — Horizon 26.3 的 macOS 启动器" },
    { id: "engine", label: "引擎", title: "引擎 — NCLR" },
    { id: "resources", label: "资源", title: "资源账本 — NCLR" },
    { id: "security", label: "边界", title: "边界 — NCLR" },
    { id: "ios", label: "iOS", title: "iOS — NCLR" },
    { id: "about", label: "关于", title: "关于 — NCLR" }
  ];

  var routeEls = {};
  document.querySelectorAll(".route").forEach(function (el) {
    routeEls[el.dataset.route] = el;
  });

  var navLinks = Array.prototype.slice.call(document.querySelectorAll("[data-route-link]"));
  var dotsWrap = document.getElementById("pagerDots");
  var prevBtn = document.getElementById("prevPage");
  var nextBtn = document.getElementById("nextPage");
  var pagerCount = document.getElementById("pagerCount");
  var railIndex = document.getElementById("railIndex");
  var railFill = document.getElementById("railFill");
  var progressBar = document.getElementById("scrollProgress");
  var cursorGlow = document.getElementById("cursorGlow");
  var menuBtn = document.getElementById("menuBtn");
  var nav = document.getElementById("nav");
  var themeToggle = document.getElementById("themeToggle");
  var rootEl = document.documentElement;
  var motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
  var reduceMotion = motionQuery.matches;
  var finePointer = window.matchMedia("(pointer: fine)").matches;

  var current = null;
  var locked = false;
  var pending = null;
  var dots = [];

  /* ---------- helpers ---------- */

  function pad(n) { return n < 10 ? "0" + n : String(n); }

  function indexOf(id) {
    for (var i = 0; i < ROUTES.length; i += 1) {
      if (ROUTES[i].id === id) return i;
    }
    return 0;
  }

  function routeFromHash() {
    var raw = (location.hash || "").replace(/^#\/?/, "").replace(/\/+$/, "").trim();
    for (var i = 0; i < ROUTES.length; i += 1) {
      if (ROUTES[i].id === raw) return raw;
    }
    return "home";
  }

  /* ---------- pager dots ---------- */

  if (dotsWrap) {
    ROUTES.forEach(function (route, i) {
      var dot = document.createElement("button");
      dot.type = "button";
      dot.setAttribute("aria-label", "第 " + (i + 1) + " 页：" + route.label);
      dot.addEventListener("click", function () {
        if (route.id !== current) {
          location.hash = "#/" + route.id;
        }
      });
      dotsWrap.appendChild(dot);
      dots.push(dot);
    });
  }

  /* ---------- chrome ---------- */

  function updateChrome(id) {
    var idx = indexOf(id);
    var total = ROUTES.length;

    navLinks.forEach(function (link) {
      if (link.dataset.routeLink === id) {
        link.setAttribute("aria-current", "page");
      } else {
        link.removeAttribute("aria-current");
      }
    });

    dots.forEach(function (dot, i) {
      dot.classList.toggle("is-active", i === idx);
      if (i === idx) dot.setAttribute("aria-current", "page");
      else dot.removeAttribute("aria-current");
    });

    if (pagerCount) pagerCount.textContent = pad(idx + 1) + " / " + pad(total);
    if (railIndex) railIndex.textContent = pad(idx + 1);
    if (railFill) railFill.style.height = ((idx + 1) / total) * 100 + "%";

    if (prevBtn) {
      prevBtn.disabled = idx === 0;
      prevBtn.querySelector("[data-pager-label]").textContent = idx > 0 ? ROUTES[idx - 1].label : "—";
    }
    if (nextBtn) {
      nextBtn.disabled = idx === total - 1;
      nextBtn.querySelector("[data-pager-label]").textContent = idx < total - 1 ? ROUTES[idx + 1].label : "—";
    }

    document.body.dataset.route = id;
    document.title = ROUTES[idx].title;
  }

  /* ---------- counters (initial paint only) ---------- */

  function animateCount(el) {
    var target = parseFloat(el.dataset.count);
    if (isNaN(target)) return;
    var decimals = parseInt(el.dataset.decimals || "0", 10);
    var format = function (value) {
      return value.toLocaleString("en-US", {
        minimumFractionDigits: decimals,
        maximumFractionDigits: decimals
      });
    };

    if (reduceMotion) { el.textContent = format(target); return; }

    var duration = 1500;
    var start = null;
    function tick(now) {
      if (start === null) start = now;
      var t = Math.min(1, (now - start) / duration);
      var eased = 1 - Math.pow(1 - t, 3);
      el.textContent = format(target * eased);
      if (t < 1) {
        window.requestAnimationFrame(tick);
      } else {
        el.textContent = format(target);
      }
    }
    el.textContent = format(0);
    window.requestAnimationFrame(tick);
  }

  /* ---------- apply route ---------- */

  function applyRoute(id, initial) {
    for (var key in routeEls) {
      if (!Object.prototype.hasOwnProperty.call(routeEls, key)) continue;
      var el = routeEls[key];
      var on = key === id;
      el.hidden = !on;
      el.classList.toggle("is-active", on);
      el.classList.remove("is-entering", "is-leaving");
    }

    current = id;
    window.scrollTo(0, 0);
    updateChrome(id);
    onScroll();

    if (initial) {
      var target = routeEls[id];
      target.querySelectorAll("[data-count]").forEach(animateCount);
    }
  }

  /* ---------- page-turn navigation ---------- */

  function navigate(id) {
    if (!routeEls[id] || id === current) return;
    if (locked) { pending = id; return; }

    var direction = indexOf(id) > indexOf(current) ? "forward" : "back";
    document.documentElement.dataset.turn = direction;
    document.body.classList.remove("is-intro");

    if (document.startViewTransition) {
      locked = true;
      var transition;
      try {
        transition = document.startViewTransition(function () {
          applyRoute(id, false);
          document.documentElement.classList.add("is-turning");
        });
      } catch (err) {
        locked = false;
        applyRoute(id, false);
        return;
      }

      transition.finished.catch(function () {}).then(function () {
        locked = false;
        document.documentElement.classList.remove("is-turning");
        if (pending && pending !== current) {
          var next = pending;
          pending = null;
          navigate(next);
        } else {
          pending = null;
        }
      });
      if (transition.ready && transition.ready.catch) transition.ready.catch(function () {});
      if (transition.updateCallbackDone && transition.updateCallbackDone.catch) {
        transition.updateCallbackDone.catch(function () {});
      }
    } else {
      locked = true;
      applyRoute(id, false);
      var el = routeEls[id];
      el.classList.add("is-entering");
      window.setTimeout(function () {
        el.classList.remove("is-entering");
        locked = false;
        if (pending && pending !== current) {
          var next = pending;
          pending = null;
          navigate(next);
        } else {
          pending = null;
        }
      }, reduceMotion ? 40 : 500);
    }
  }

  function goRelative(delta) {
    var idx = indexOf(current) + delta;
    if (idx < 0 || idx >= ROUTES.length) return;
    var id = ROUTES[idx].id;
    if (id === current) return;
    location.hash = "#/" + id;
  }

  /* ---------- theme ---------- */

  function storedTheme() {
    try {
      var value = localStorage.getItem("nclr-theme");
      return value === "light" || value === "dark" ? value : null;
    } catch (err) {
      return null;
    }
  }

  function syncThemeUI() {
    var light = rootEl.dataset.theme === "light";
    if (themeToggle) {
      themeToggle.setAttribute("aria-pressed", light ? "true" : "false");
      themeToggle.setAttribute("aria-label", light ? "切换到黑夜模式" : "切换到白天模式");
      themeToggle.title = light ? "切换到黑夜模式" : "切换到白天模式";
    }
    var label = document.querySelector(".theme-label");
    if (label) label.textContent = light ? "白天" : "黑夜";
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", light ? "#fbfbf9" : "#050505");
  }

  function setTheme(theme, persist) {
    rootEl.dataset.theme = theme;
    if (persist) {
      try { localStorage.setItem("nclr-theme", theme); } catch (err) {}
    }
    syncThemeUI();
  }

  if (themeToggle) {
    themeToggle.addEventListener("click", function () {
      var next = rootEl.dataset.theme === "light" ? "dark" : "light";
      rootEl.classList.add("theme-shift");
      setTheme(next, true);
      window.setTimeout(function () { rootEl.classList.remove("theme-shift"); }, 420);
    });
  }

  syncThemeUI();

  if (motionQuery.addEventListener) {
    motionQuery.addEventListener("change", function (event) {
      reduceMotion = event.matches;
    });
  }

  /* Follow the OS theme until the visitor makes a choice. */
  var schemeQuery = window.matchMedia("(prefers-color-scheme: light)");
  var onSchemeChange = function (event) {
    if (!storedTheme()) setTheme(event.matches ? "light" : "dark", false);
  };
  if (schemeQuery.addEventListener) schemeQuery.addEventListener("change", onSchemeChange);

  /* ---------- menu ---------- */

  function closeMenu() {
    if (!menuBtn || !nav) return;
    var wasOpen = nav.classList.contains("open");
    var hadFocus = wasOpen && nav.contains(document.activeElement);
    nav.classList.remove("open");
    menuBtn.setAttribute("aria-expanded", "false");
    menuBtn.setAttribute("aria-label", "打开菜单");
    if (hadFocus && window.innerWidth <= 1080) menuBtn.focus({ preventScroll: true });
  }

  if (menuBtn && nav) {
    menuBtn.addEventListener("click", function (event) {
      event.stopPropagation();
      var open = nav.classList.toggle("open");
      menuBtn.setAttribute("aria-expanded", open ? "true" : "false");
      menuBtn.setAttribute("aria-label", open ? "关闭菜单" : "打开菜单");
    });

    document.addEventListener("click", function (event) {
      if (!nav.classList.contains("open")) return;
      if (nav.contains(event.target) || menuBtn.contains(event.target)) return;
      closeMenu();
    });

    window.addEventListener("resize", function () {
      if (window.innerWidth > 1080) closeMenu();
    });
  }

  /* ---------- links, pager, keyboard, swipe ---------- */

  document.addEventListener("click", function (event) {
    var link = event.target.closest ? event.target.closest('a[href^="#/"]') : null;
    if (!link) return;
    var id = link.getAttribute("href").replace(/^#\//, "");
    if (!routeEls[id]) return;
    closeMenu();

    if (id === current) {
      event.preventDefault();
      window.scrollTo({ top: 0, behavior: reduceMotion ? "auto" : "smooth" });
    }
  });

  if (prevBtn) prevBtn.addEventListener("click", function () { goRelative(-1); });
  if (nextBtn) nextBtn.addEventListener("click", function () { goRelative(1); });

  window.addEventListener("keydown", function (event) {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    var tag = event.target && event.target.tagName;
    if (event.target && (event.target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(tag))) return;

    if (event.key === "ArrowRight") {
      event.preventDefault();
      goRelative(1);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      goRelative(-1);
    } else if (event.key === "Escape") {
      closeMenu();
    }
  });

  var touchStartX = 0;
  var touchStartY = 0;
  var touching = false;

  window.addEventListener("touchstart", function (event) {
    if (event.touches.length !== 1) return;
    touchStartX = event.touches[0].clientX;
    touchStartY = event.touches[0].clientY;
    touching = true;
  }, { passive: true });

  window.addEventListener("touchend", function (event) {
    if (!touching) return;
    touching = false;
    var touch = event.changedTouches[0];
    var dx = touch.clientX - touchStartX;
    var dy = touch.clientY - touchStartY;
    if (Math.abs(dx) > 80 && Math.abs(dx) > Math.abs(dy) * 1.5) {
      goRelative(dx < 0 ? 1 : -1);
    }
  }, { passive: true });

  /* ---------- hash routing ---------- */

  window.addEventListener("hashchange", function () {
    var id = routeFromHash();
    if (!location.hash || location.hash === "#") {
      try { history.replaceState(null, "", "#/home"); } catch (err) {}
    }
    navigate(id);
  });

  /* ---------- scroll progress ---------- */

  function onScroll() {
    if (!progressBar) return;
    var doc = document.documentElement;
    var max = doc.scrollHeight - window.innerHeight;
    var ratio = max > 0 ? Math.min(1, Math.max(0, window.scrollY / max)) : 0;
    progressBar.style.width = (ratio * 100).toFixed(2) + "%";
  }

  window.addEventListener("scroll", onScroll, { passive: true });
  window.addEventListener("resize", onScroll);

  /* ---------- reveal on scroll ---------- */

  var revealEls = Array.prototype.slice.call(document.querySelectorAll(".reveal"));
  if ("IntersectionObserver" in window && !reduceMotion) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add("in");
          io.unobserve(entry.target);
        }
      });
    }, { threshold: 0.08, rootMargin: "0px 0px -6% 0px" });
    revealEls.forEach(function (el) { io.observe(el); });
  } else {
    revealEls.forEach(function (el) { el.classList.add("in"); });
  }

  /* ---------- spotlight ---------- */

  document.querySelectorAll("[data-spot]").forEach(function (card) {
    card.addEventListener("mousemove", function (event) {
      var rect = card.getBoundingClientRect();
      card.style.setProperty("--mx", (event.clientX - rect.left).toFixed(1) + "px");
      card.style.setProperty("--my", (event.clientY - rect.top).toFixed(1) + "px");
    });
  });

  /* ---------- screenshot tilt ---------- */

  if (finePointer && !reduceMotion) {
    document.querySelectorAll("[data-tilt]").forEach(function (el) {
      el.addEventListener("mousemove", function (event) {
        var rect = el.getBoundingClientRect();
        var px = (event.clientX - rect.left) / rect.width - 0.5;
        var py = (event.clientY - rect.top) / rect.height - 0.5;
        el.style.transition = "transform 0.12s ease-out";
        el.style.transform = "perspective(1200px) rotateX(" + (-py * 4.5).toFixed(2) + "deg) rotateY(" + (px * 4.5).toFixed(2) + "deg) translateY(-3px)";
      });
      el.addEventListener("mouseleave", function () {
        el.style.transition = "transform 0.55s cubic-bezier(0.2,0.7,0.2,1)";
        el.style.transform = "";
      });
    });
  }

  /* ---------- cursor glow ---------- */

  if (finePointer && !reduceMotion && cursorGlow) {
    var tx = window.innerWidth * 0.5;
    var ty = window.innerHeight * 0.32;
    var cx = tx;
    var cy = ty;
    var shown = false;

    window.addEventListener("mousemove", function (event) {
      tx = event.clientX;
      ty = event.clientY;
      if (!shown) {
        shown = true;
        cursorGlow.classList.add("is-on");
      }
    }, { passive: true });

    (function glowLoop() {
      cx += (tx - cx) * 0.12;
      cy += (ty - cy) * 0.12;
      cursorGlow.style.transform = "translate3d(" + (cx - 280).toFixed(1) + "px," + (cy - 280).toFixed(1) + "px,0)";
      window.requestAnimationFrame(glowLoop);
    })();
  }

  /* ---------- initialise ---------- */

  var initial = routeFromHash();
  if (!location.hash) {
    try { history.replaceState(null, "", "#/home"); } catch (err) {}
  }

  document.body.classList.add("is-intro");
  applyRoute(initial, true);

  window.setTimeout(function () {
    document.body.classList.remove("is-intro");
  }, 2400);
})();
