/* plumber — общий скрипт сайта: тема, кнопки «копировать», подсветка навигации.
   Без зависимостей; сайт должен работать и с file://, и офлайн. */

(function () {
  "use strict";

  // ---- тема ------------------------------------------------------------
  var KEY = "plumber.theme";
  function apply(theme) {
    if (theme === "light" || theme === "dark") {
      document.documentElement.setAttribute("data-theme", theme);
    } else {
      document.documentElement.removeAttribute("data-theme");
    }
  }
  try { apply(localStorage.getItem(KEY)); } catch (e) { /* приватный режим */ }

  function setupTheme() {
    var btn = document.getElementById("themeBtn");
    if (!btn) return;
    function current() {
      var attr = document.documentElement.getAttribute("data-theme");
      if (attr) return attr;
      return window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    }
    function label() {
      btn.textContent = current() === "light" ? "Тёмная" : "Светлая";
      btn.setAttribute("aria-label", "Переключить тему");
    }
    label();
    btn.addEventListener("click", function () {
      var next = current() === "light" ? "dark" : "light";
      apply(next);
      try { localStorage.setItem(KEY, next); } catch (e) { /* игнорируем */ }
      label();
    });
  }

  // ---- кнопки копирования в блоках кода --------------------------------
  function setupCopy() {
    var blocks = document.querySelectorAll("pre");
    for (var i = 0; i < blocks.length; i++) {
      (function (pre) {
        var btn = document.createElement("button");
        btn.className = "copy";
        btn.type = "button";
        btn.textContent = "копировать";
        btn.addEventListener("click", function () {
          var text = pre.querySelector("code") ? pre.querySelector("code").innerText : pre.innerText;
          var done = function () {
            btn.textContent = "скопировано";
            setTimeout(function () { btn.textContent = "копировать"; }, 1400);
          };
          if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(text).then(done, function () { fallback(text, done); });
          } else {
            fallback(text, done);
          }
        });
        pre.appendChild(btn);
      })(blocks[i]);
    }
  }
  function fallback(text, done) {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try { document.execCommand("copy"); done(); } catch (e) { /* ничего */ }
    document.body.removeChild(area);
  }

  // ---- подсветка текущей страницы в меню -------------------------------
  function markCurrent() {
    var here = location.pathname.split("/").pop() || "index.html";
    var links = document.querySelectorAll(".topnav nav a");
    for (var i = 0; i < links.length; i++) {
      var href = links[i].getAttribute("href");
      if (!href) continue;
      var target = href.split("/").pop();
      if (target === here || (here === "" && target === "index.html")) {
        links[i].setAttribute("aria-current", "page");
      }
    }
  }

  document.addEventListener("DOMContentLoaded", function () {
    setupTheme();
    setupCopy();
    markCurrent();
  });
})();
