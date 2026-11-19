/* Same-origin bridge for the official image page.
   Rewrites the four official hosts to this server and attaches the local access key.
   The upstream token never enters the page. */
(function () {
  var HOSTS = ["image.novelai.net", "api.novelai.net", "text.novelai.net", "explore.novelai.net"];
  var KEY_NAME = "wb-access";

  function accessKey() {
    try { return sessionStorage.getItem(KEY_NAME) || ""; } catch (e) { return ""; }
  }

  function map(url) {
    try {
      var u = new URL(url, location.href);
      if (HOSTS.indexOf(u.host) >= 0) return u.pathname + u.search;
    } catch (e) {}
    return url;
  }

  function needsAuth(url) {
    try {
      var u = new URL(url, location.href);
      if (u.origin !== location.origin) return false;
      return /^\/(ai|user|usage|public|v\d)\//.test(u.pathname);
    } catch (e) {
      return false;
    }
  }

  function asJwt() {
    var payload = btoa(JSON.stringify({ id: "local", iat: Math.floor(Date.now() / 1000) }));
    return "wb." + payload + ".wb";
  }

  function rememberSession() {
    var key = accessKey();
    if (!key) return;
    try {
      var ek = localStorage.getItem("wb-ek");
      if (!ek) {
        var raw = new Uint8Array(32);
        crypto.getRandomValues(raw);
        ek = btoa(String.fromCharCode.apply(null, raw));
        localStorage.setItem("wb-ek", ek);
      }
      var want = JSON.stringify({ auth_token: asJwt(), encryption_key: ek });
      localStorage.setItem("session", want);
      sessionStorage.setItem("session", want);
    } catch (e) {}
  }

  var of = window.fetch;
  window.fetch = function (input, init) {
    var url = typeof input === "string" ? input : (input && input.url) || "";
    var mapped = map(url);
    var key = accessKey();
    if (mapped === url && !(key && needsAuth(url))) return of.apply(window, arguments);
    init = init ? Object.assign({}, init) : {};
    var headers = new Headers(init.headers || (typeof input !== "string" && input && input.headers) || {});
    if (key) headers.set("authorization", "Bearer " + key);
    init.headers = headers;
    init.credentials = "same-origin";
    return of.call(window, mapped, init);
  };

  var XO = XMLHttpRequest.prototype.open;
  var XS = XMLHttpRequest.prototype.setRequestHeader;
  var OS = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    var mapped = map(url);
    this.__wbAuth = mapped !== url || needsAuth(url);
    return XO.apply(this, [method, mapped].concat(Array.prototype.slice.call(arguments, 2)));
  };
  XMLHttpRequest.prototype.setRequestHeader = function (name, value) {
    if (this.__wbAuth && String(name).toLowerCase() === "authorization") return;
    return XS.call(this, name, value);
  };
  XMLHttpRequest.prototype.send = function (body) {
    if (this.__wbAuth && accessKey()) {
      try { XS.call(this, "authorization", "Bearer " + accessKey()); } catch (e) {}
    }
    return OS.call(this, body);
  };

  rememberSession();

  function askForKey() {
    if (accessKey() || document.getElementById("wb-unlock")) return;
    var style = document.createElement("style");
    style.textContent = "#wb-unlock{position:fixed;inset:0;z-index:2147483646;display:flex;align-items:center;justify-content:center;background:rgba(8,8,16,.72);font:16px/1.4 system-ui,sans-serif}"
      + "#wb-unlock form{width:min(420px,calc(100vw - 32px));padding:24px;border-radius:12px;background:#16161f;color:#f4f4f8;box-shadow:0 16px 40px rgba(0,0,0,.35)}"
      + "#wb-unlock h1{margin:0 0 8px;font-size:18px}#wb-unlock p{margin:0 0 16px;color:#b7b7c7;font-size:14px}"
      + "#wb-unlock input{box-sizing:border-box;width:100%;padding:10px 12px;border-radius:8px;border:1px solid #3a3a4a;background:#0e0e14;color:inherit}"
      + "#wb-unlock button{margin-top:12px;padding:10px 14px;border:0;border-radius:8px;background:#6d5efc;color:#fff;font:600 14px system-ui,sans-serif;cursor:pointer}";
    document.documentElement.appendChild(style);
    var wrap = document.createElement("div");
    wrap.id = "wb-unlock";
    wrap.innerHTML = '<form><h1>本机访问口令</h1><p>Local access key. It stays in this tab and is not your upstream token.</p><input type="password" name="key" autocomplete="current-password" required minlength="16"><button type="submit">继续</button></form>';
    document.documentElement.appendChild(wrap);
    wrap.querySelector("form").addEventListener("submit", function (ev) {
      ev.preventDefault();
      var value = new FormData(ev.target).get("key");
      if (!value || String(value).length < 16) return;
      sessionStorage.setItem(KEY_NAME, String(value));
      location.reload();
    });
  }

  if (!accessKey()) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", askForKey);
    else askForKey();
  }

  var HIDE_TEXTS = [
    "Create an account and start for free",
    "Your subscription renewal payment has failed",
    "Confirm your email",
    "We’ve updated our Terms of Service",
    "We've updated our Terms of Service"
  ];
  var SPLASH_LOAD = ["Load", "载入"];
  var SPLASH_DISMISS = ["Close", "Dismiss", "Skip", "Continue", "Got it", "OK", "关闭", "跳过", "知道了"];

  function splashKind(text) {
    if (!text) return "";
    if (/Load a previous session|Restore previous session|上次会话/.test(text)) return "session";
    if (/What.?s New in NovelAI|Here.?s what.?s new|Start generating now|Generate Images For Free|Start for Free!?/.test(text)) return "promo";
    return "";
  }

  function splashClick(root, labels) {
    var buttons = root.querySelectorAll("button");
    for (var i = 0; i < buttons.length; i++) {
      var t = (buttons[i].textContent || "").replace(/\s+/g, " ").trim();
      for (var j = 0; j < labels.length; j++) {
        if (t === labels[j]) { buttons[i].click(); return true; }
      }
    }
    return false;
  }

  function dismissSplash() {
    if (!document.body) return;
    var nodes = document.querySelectorAll('[role="dialog"][aria-modal="true"], dialog[open]');
    for (var i = 0; i < nodes.length; i++) {
      var el = nodes[i];
      if (el.id === "wb-unlock" || el.getAttribute("data-wb-splash")) continue;
      var kind = splashKind(el.innerText || "");
      if (!kind) continue;
      var ok = false;
      if (kind === "session") {
        ok = splashClick(el, SPLASH_LOAD);
        if (!ok) continue;
      } else {
        var close = el.querySelector(".modal-close");
        if (close) { close.click(); ok = true; }
        else ok = splashClick(el, SPLASH_DISMISS);
        if (!ok && el.parentElement) el.parentElement.click();
      }
      el.setAttribute("data-wb-splash", kind);
    }
  }

  function hideBanners() {
    if (!document.body) return;
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    var hits = [];
    while (walker.nextNode()) {
      var t = (walker.currentNode.nodeValue || "").trim();
      for (var i = 0; i < HIDE_TEXTS.length; i++) {
        if (t.indexOf(HIDE_TEXTS[i]) >= 0) { hits.push(walker.currentNode.parentElement); break; }
      }
    }
    for (var j = 0; j < hits.length; j++) {
      var el = hits[j];
      for (var k = 0; k < 4 && el && el !== document.body; k++) {
        var r = el.getBoundingClientRect();
        if (r.width > window.innerWidth * 0.6 && r.height > 16 && r.height < 120) {
          el.style.setProperty("display", "none", "important");
          break;
        }
        el = el.parentElement;
      }
    }
  }

  var WB_LANG_KEY = "wb_lang";
  var wbLang = "en";
  var wbMap = {};
  var wbKeep = {};
  var wbPainting = false;
  try {
    var saved = localStorage.getItem(WB_LANG_KEY);
    if (saved === "zh" || saved === "en") wbLang = saved;
  } catch (e) {}

  function wbSkip(el) {
    if (!el || el.nodeType !== 1) return true;
    if (el.id === "wb-lang-toggle" || el.id === "wb-unlock") return true;
    if (el.closest && (el.closest("#wb-lang-toggle") || el.closest("#wb-unlock"))) return true;
    var tag = (el.tagName || "").toLowerCase();
    return tag === "script" || tag === "style" || tag === "textarea" || tag === "input";
  }

  function wbRender(s) {
    if (!s || wbKeep[s]) return s;
    if (wbMap[s]) return wbMap[s];
    var m = /^Generate (\d+) Images?$/.exec(s);
    if (m) return "生成 " + m[1] + " 张";
    return s;
  }

  function wbWalk(root) {
    if (!root || wbLang !== "zh") return;
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: function (node) {
        if (wbSkip(node.parentElement)) return NodeFilter.FILTER_REJECT;
        if (!node.nodeValue || !node.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    var node;
    while ((node = walker.nextNode())) {
      var raw = node.nodeValue;
      var key = raw.trim();
      var next = wbRender(key);
      if (next === key) continue;
      if (node.wbSrc == null) node.wbSrc = raw;
      node.nodeValue = raw.match(/^\s*/)[0] + next + raw.match(/\s*$/)[0];
    }
  }

  function wbRestore(root) {
    if (!root) return;
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    var node;
    while ((node = walker.nextNode())) {
      if (node.wbSrc != null) {
        node.nodeValue = node.wbSrc;
        node.wbSrc = null;
      }
    }
  }

  function wbApply() {
    if (wbPainting || !document.body) return;
    wbPainting = true;
    try {
      if (wbLang === "zh") wbWalk(document.body);
      else wbRestore(document.body);
    } finally {
      wbPainting = false;
    }
  }

  function wbSetLang(next) {
    wbLang = next === "zh" ? "zh" : "en";
    try { localStorage.setItem(WB_LANG_KEY, wbLang); } catch (e) {}
    var btn = document.getElementById("wb-lang-toggle");
    if (btn) {
      btn.textContent = wbLang === "zh" ? "中" : "EN";
      btn.title = wbLang === "zh" ? "中文" : "English";
    }
    wbApply();
  }

  function wbPlaceToggle() {
    var btn = document.getElementById("wb-lang-toggle");
    if (!btn) return;
    var menus = document.querySelectorAll('button[aria-label="menu"]');
    var menu = null;
    for (var i = 0; i < menus.length; i++) {
      var rect = menus[i].getBoundingClientRect();
      if (rect.width && rect.height) { menu = menus[i]; break; }
    }
    if (menu && btn.parentElement !== menu.parentElement) menu.parentElement.insertBefore(btn, menu);
    btn.style.visibility = menu ? "visible" : "hidden";
  }

  function wbMountToggle() {
    if (document.getElementById("wb-lang-toggle")) return;
    var btn = document.createElement("button");
    btn.id = "wb-lang-toggle";
    btn.type = "button";
    btn.textContent = wbLang === "zh" ? "中" : "EN";
    btn.style.cssText = "position:relative;flex:0 0 36px;align-self:center;width:36px;height:28px;margin:0 4px 0 0;border:1px solid #3a3560;border-radius:6px;background:#1b1836;color:#f4f0ff;font:700 12px/1 system-ui,sans-serif;cursor:pointer;visibility:hidden";
    btn.addEventListener("click", function (ev) {
      ev.preventDefault();
      ev.stopPropagation();
      wbSetLang(wbLang === "zh" ? "en" : "zh");
    });
    (document.body || document.documentElement).appendChild(btn);
    wbPlaceToggle();
  }

  function watchPage() {
    hideBanners();
    dismissSplash();
    wbPlaceToggle();
  }

  function bootI18n(dict) {
    var keep = (dict && dict.keep) || [];
    var map = (dict && dict.map) || {};
    wbKeep = {};
    for (var i = 0; i < keep.length; i++) wbKeep[keep[i]] = 1;
    wbMap = map;
    wbMountToggle();
    if (wbLang === "zh") wbApply();
    new MutationObserver(watchPage).observe(document.documentElement, { childList: true, subtree: true, characterData: true });
    watchPage();
  }

  function start() {
    watchPage();
    fetch("/workbench-i18n.json", { cache: "no-store", credentials: "same-origin" })
      .then(function (r) { if (!r.ok) throw new Error("missing"); return r.json(); })
      .then(bootI18n)
      .catch(function () { bootI18n({ keep: [], map: {} }); });
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
