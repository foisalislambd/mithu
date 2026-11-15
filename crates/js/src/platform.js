// Host classes cover the constructors Boa has to see for `instanceof`.
// The rest of the commonly used window surface lives here so a missing
// name fails in one place instead of one site at a time.
(function () {
  function unitValue(value, unit) {
    var numeric = Number(value);
    return {
      value: numeric,
      unit: unit,
      toString: function () {
        if (unit === "percent") return numeric + "%";
        if (unit === "number") return String(numeric);
        return numeric + unit;
      }
    };
  }
  if (typeof CSS !== "object" || CSS === null) {
    var css = {
      supports: function () { return true; },
      escape: function (value) {
        var text = String(value);
        var out = "";
        for (var i = 0; i < text.length; i++) {
          var code = text.charCodeAt(i);
          var ch = text.charAt(i);
          if (code === 0) { out += "\uFFFD"; continue; }
          if (i === 0 && code >= 0x30 && code <= 0x39) {
            out += "\\" + code.toString(16) + " ";
            continue;
          }
          if (code >= 0x80 || (code >= 48 && code <= 57) || (code >= 65 && code <= 90) || (code >= 97 && code <= 122) || code === 45 || code === 95) {
            out += ch;
          } else {
            out += "\\" + ch;
          }
        }
        return out;
      },
      registerProperty: function () {},
      highlights: {
        size: 0,
        set: function () {},
        get: function () { return undefined; },
        has: function () { return false; },
        delete: function () { return false; },
        clear: function () {}
      }
    };
    ["number", "percent", "px", "em", "rem", "vh", "vw", "vmin", "vmax", "dvh", "svh", "lvh", "vi", "vb", "ch", "ex", "cap", "lh", "cqw", "cqh", "deg", "rad", "grad", "turn", "s", "ms", "Hz", "kHz", "fr", "cm", "mm", "in", "pt", "pc", "Q"].forEach(function (unit) {
      css[unit] = function (value) { return unitValue(value, unit); };
    });
    globalThis.CSS = css;
  }

  function defineMethod(ctorName, name, fn) {
    var ctor = globalThis[ctorName];
    if (!ctor || !ctor.prototype || typeof ctor.prototype[name] === "function") return;
    ctor.prototype[name] = fn;
  }
  defineMethod("HTMLDialogElement", "show", function () {
    this.setAttribute("open", "");
  });
  defineMethod("HTMLDialogElement", "showModal", function () {
    this.setAttribute("open", "");
  });
  defineMethod("HTMLDialogElement", "close", function (value) {
    if (value !== undefined) this.returnValue = String(value);
    this.removeAttribute("open");
    if (typeof this.dispatchEvent === "function") this.dispatchEvent(new Event("close"));
  });
  if (globalThis.HTMLDialogElement && HTMLDialogElement.prototype) {
    if (!Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "open")) {
      Object.defineProperty(HTMLDialogElement.prototype, "open", {
        configurable: true,
        get: function () { return this.hasAttribute("open"); },
        set: function (value) {
          if (value) this.setAttribute("open", "");
          else this.removeAttribute("open");
        }
      });
    }
    if (!Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype, "returnValue")) {
      Object.defineProperty(HTMLDialogElement.prototype, "returnValue", {
        configurable: true,
        get: function () { return this.__returnValue || ""; },
        set: function (value) { this.__returnValue = String(value); }
      });
    }
  }
  defineMethod("HTMLSlotElement", "assignedNodes", function () {
    var nodes = [];
    var child = this.firstChild;
    while (child) { nodes.push(child); child = child.nextSibling; }
    return nodes;
  });
  defineMethod("HTMLSlotElement", "assignedElements", function () {
    return this.assignedNodes().filter(function (node) { return node.nodeType === 1; });
  });
  defineMethod("HTMLSlotElement", "assign", function () {});

  function Headers(init) {
    if (!(this instanceof Headers)) return new Headers(init);
    this._list = [];
    var self = this;
    if (typeof init === "string") {
      init.split(/\n/).forEach(function (line) {
        var split = line.indexOf(":");
        if (split > 0) self.append(line.slice(0, split), line.slice(split + 1));
      });
    } else if (init && typeof init.forEach === "function") {
      init.forEach(function (value, name) { self.append(name, value); });
    } else if (init && typeof init === "object") {
      Object.keys(init).forEach(function (name) { self.append(name, init[name]); });
    }
  }
  Headers.prototype.append = function (name, value) {
    this._list.push([String(name).toLowerCase(), String(value).trim()]);
  };
  Headers.prototype.delete = function (name) {
    name = String(name).toLowerCase();
    this._list = this._list.filter(function (pair) { return pair[0] !== name; });
  };
  Headers.prototype.get = function (name) {
    name = String(name).toLowerCase();
    var found = [];
    this._list.forEach(function (pair) { if (pair[0] === name) found.push(pair[1]); });
    return found.length ? found.join(", ") : null;
  };
  Headers.prototype.getSetCookie = function () {
    return this._list.filter(function (pair) { return pair[0] === "set-cookie"; }).map(function (pair) { return pair[1]; });
  };
  Headers.prototype.has = function (name) { return this.get(name) != null; };
  Headers.prototype.set = function (name, value) {
    this.delete(name);
    this.append(name, value);
  };
  Headers.prototype.forEach = function (callback, thisArg) {
    var self = this;
    this._list.forEach(function (pair) { callback.call(thisArg, pair[1], pair[0], self); });
  };
  Headers.prototype.entries = function () { return this._list[Symbol.iterator](); };
  Headers.prototype.keys = function () { return this._list.map(function (pair) { return pair[0]; })[Symbol.iterator](); };
  Headers.prototype.values = function () { return this._list.map(function (pair) { return pair[1]; })[Symbol.iterator](); };
  Headers.prototype[Symbol.iterator] = function () { return this.entries(); };
  globalThis.Headers = Headers;

  function bytesOf(text) {
    try { return new TextEncoder().encode(text).length; } catch (e) { return text.length; }
  }
  function Blob(parts, options) {
    if (!(this instanceof Blob)) return new Blob(parts, options);
    var text = "";
    (parts || []).forEach(function (part) {
      if (typeof part === "string") text += part;
      else if (part instanceof ArrayBuffer) text += new TextDecoder().decode(new Uint8Array(part));
      else if (ArrayBuffer.isView(part)) text += new TextDecoder().decode(part);
      else if (part && typeof part.text === "function" && part._text != null) text += part._text;
      else text += String(part);
    });
    this._text = text;
    this.size = bytesOf(text);
    this.type = (options && options.type) || "";
  }
  Blob.prototype.text = function () { return Promise.resolve(this._text); };
  Blob.prototype.arrayBuffer = function () {
    return Promise.resolve(new TextEncoder().encode(this._text).buffer);
  };
  Blob.prototype.slice = function (start, end, type) {
    return new Blob([this._text.slice(start, end)], { type: type || this.type });
  };
  Blob.prototype.stream = function () {
    var text = this._text;
    return new ReadableStream({
      start: function (controller) {
        if (text) controller.enqueue(text);
        controller.close();
      }
    });
  };
  globalThis.Blob = Blob;
  function File(parts, name, options) {
    if (!(this instanceof File)) return new File(parts, name, options);
    Blob.call(this, parts, options);
    this.name = String(name);
    this.lastModified = (options && options.lastModified) || Date.now();
  }
  File.prototype = Object.create(Blob.prototype);
  File.prototype.constructor = File;
  globalThis.File = File;

  function FormData(form) {
    if (!(this instanceof FormData)) return new FormData(form);
    this._entries = [];
    if (form && form.elements) {
      for (var i = 0; i < form.elements.length; i++) {
        var field = form.elements[i];
        if (field && field.name && !field.disabled) this.append(field.name, field.value == null ? "" : String(field.value));
      }
    }
  }
  FormData.prototype.append = function (name, value) { this._entries.push([String(name), value]); };
  FormData.prototype.delete = function (name) {
    name = String(name);
    this._entries = this._entries.filter(function (entry) { return entry[0] !== name; });
  };
  FormData.prototype.get = function (name) {
    name = String(name);
    for (var i = 0; i < this._entries.length; i++) if (this._entries[i][0] === name) return this._entries[i][1];
    return null;
  };
  FormData.prototype.getAll = function (name) {
    name = String(name);
    return this._entries.filter(function (entry) { return entry[0] === name; }).map(function (entry) { return entry[1]; });
  };
  FormData.prototype.has = function (name) { return this.get(name) != null; };
  FormData.prototype.set = function (name, value) { this.delete(name); this.append(name, value); };
  FormData.prototype.forEach = function (callback, thisArg) {
    var self = this;
    this._entries.forEach(function (entry) { callback.call(thisArg, entry[1], entry[0], self); });
  };
  FormData.prototype.entries = function () { return this._entries.map(function (entry) { return [entry[0], entry[1]]; })[Symbol.iterator](); };
  FormData.prototype.keys = function () { return this._entries.map(function (entry) { return entry[0]; })[Symbol.iterator](); };
  FormData.prototype.values = function () { return this._entries.map(function (entry) { return entry[1]; })[Symbol.iterator](); };
  FormData.prototype[Symbol.iterator] = function () { return this.entries(); };
  globalThis.FormData = FormData;

  function absoluteUrl(value) {
    try { return new URL(String(value), location.href).href; } catch (e) { return String(value); }
  }
  function Request(input, init) {
    if (!(this instanceof Request)) return new Request(input, init);
    init = init || {};
    var url = typeof input === "string" ? input : (input && input.url) || "";
    this.url = absoluteUrl(url);
    this.method = String((init.method || (input && input.method) || "GET")).toUpperCase();
    this.headers = new Headers(init.headers || (input && input.headers));
    this._body = init.body == null ? "" : (typeof init.body === "string" ? init.body : (init.body._text || String(init.body)));
    this.bodyUsed = false;
    this.signal = init.signal || (input && input.signal) || null;
    this.mode = init.mode || "cors";
    this.credentials = init.credentials || "same-origin";
    this.cache = init.cache || "default";
    this.redirect = init.redirect || "follow";
    this.referrer = init.referrer || "about:client";
    this.integrity = init.integrity || "";
    this.destination = "";
    this.body = null;
  }
  Request.prototype.text = function () { this.bodyUsed = true; return Promise.resolve(this._body); };
  Request.prototype.json = function () { return this.text().then(function (text) { return JSON.parse(text); }); };
  Request.prototype.arrayBuffer = function () { return this.text().then(function (text) { return new TextEncoder().encode(text).buffer; }); };
  Request.prototype.blob = function () { return this.text().then(function (text) { return new Blob([text]); }); };
  Request.prototype.formData = function () { this.bodyUsed = true; return Promise.resolve(new FormData()); };
  Request.prototype.clone = function () {
    return new Request(this.url, { method: this.method, headers: this.headers, body: this._body, signal: this.signal, mode: this.mode });
  };
  globalThis.Request = Request;

  function Response(body, init) {
    if (!(this instanceof Response)) return new Response(body, init);
    init = init || {};
    this.status = init.status === undefined ? 200 : Number(init.status);
    this.ok = this.status >= 200 && this.status < 300;
    this.statusText = init.statusText || (this.ok ? "OK" : "");
    this.url = init.url || "";
    this.redirected = !!init.redirected;
    this.type = init.type || "basic";
    this.headers = init.headers instanceof Headers ? init.headers : new Headers(init.headers);
    this._body = body == null ? "" : (typeof body === "string" ? body : (body._text || String(body)));
    this.bodyUsed = false;
    var text = this._body;
    this.body = new ReadableStream({
      start: function (controller) {
        if (text) controller.enqueue(text);
        controller.close();
      }
    });
  }
  Response.prototype.text = function () { this.bodyUsed = true; return Promise.resolve(this._body); };
  Response.prototype.json = function () { return this.text().then(function (text) { return JSON.parse(text); }); };
  Response.prototype.arrayBuffer = function () { return this.text().then(function (text) { return new TextEncoder().encode(text).buffer; }); };
  Response.prototype.blob = function () {
    var type = this.headers.get("content-type") || "";
    return this.text().then(function (text) { return new Blob([text], { type: type }); });
  };
  Response.prototype.formData = function () { this.bodyUsed = true; return Promise.resolve(new FormData()); };
  Response.prototype.clone = function () {
    return new Response(this._body, { status: this.status, statusText: this.statusText, headers: this.headers, url: this.url, type: this.type });
  };
  Response.json = function (data, init) {
    var headers = new Headers(init && init.headers);
    if (!headers.has("content-type")) headers.set("content-type", "application/json");
    return new Response(JSON.stringify(data), { status: init && init.status, statusText: init && init.statusText, headers: headers });
  };
  Response.error = function () { return new Response("", { status: 0, type: "error" }); };
  Response.redirect = function (url, status) {
    return new Response("", { status: status || 302, headers: { location: String(url) } });
  };
  globalThis.Response = Response;

  if (typeof AbortSignal.any !== "function") {
    AbortSignal.any = function (signals) {
      var controller = new AbortController();
      for (var i = 0; i < signals.length; i++) {
        var signal = signals[i];
        if (!signal) continue;
        if (signal.aborted) { controller.abort(signal.reason); break; }
        signal.addEventListener("abort", function () { controller.abort(this.reason); });
      }
      return controller.signal;
    };
  }
  AbortSignal.timeout = function (milliseconds) {
    var controller = new AbortController();
    setTimeout(function () {
      var reason = new Error("The operation was aborted due to timeout");
      reason.name = "TimeoutError";
      controller.abort(reason);
    }, Number(milliseconds) || 0);
    return controller.signal;
  };

  if (typeof navigation !== "object" || navigation === null) {
    var nav = {
      canGoBack: false,
      canGoForward: false,
      transition: null,
      addEventListener: function () {},
      removeEventListener: function () {},
      entries: function () { return [nav.currentEntry]; },
      navigate: function (url) {
        location.assign(typeof url === "string" ? url : (url && url.url) || location.href);
        return { committed: Promise.resolve(nav.currentEntry), finished: Promise.resolve(nav.currentEntry) };
      },
      back: function () { history.back(); return { committed: Promise.resolve(), finished: Promise.resolve() }; },
      forward: function () { history.forward(); return { committed: Promise.resolve(), finished: Promise.resolve() }; },
      reload: function () { location.reload(); return { committed: Promise.resolve(), finished: Promise.resolve() }; },
      traverseTo: function () { return { committed: Promise.resolve(), finished: Promise.resolve() }; },
      updateCurrentEntry: function (options) { history.replaceState(options && options.state, ""); }
    };
    Object.defineProperty(nav, "currentEntry", {
      configurable: true,
      get: function () {
        return {
          url: location.href,
          key: "current",
          id: "current",
          index: 0,
          sameDocument: true,
          getState: function () { return history.state; }
        };
      }
    });
    globalThis.navigation = nav;
  }

  if (typeof WeakRef !== "function") {
    globalThis.WeakRef = function WeakRef(target) {
      if (!(this instanceof WeakRef)) throw new TypeError("WeakRef constructor must be called with new");
      this.deref = function () { return target; };
    };
  }
  if (typeof FinalizationRegistry !== "function") {
    globalThis.FinalizationRegistry = function FinalizationRegistry() {
      if (!(this instanceof FinalizationRegistry)) throw new TypeError("FinalizationRegistry constructor must be called with new");
    };
    FinalizationRegistry.prototype.register = function () {};
    FinalizationRegistry.prototype.unregister = function () { return false; };
  }

  if (typeof Array.prototype.findLast !== "function") {
    Array.prototype.findLast = function (predicate, thisArg) {
      for (var i = this.length - 1; i >= 0; i--) {
        if (i in this && predicate.call(thisArg, this[i], i, this)) return this[i];
      }
      return undefined;
    };
  }
  if (typeof Array.prototype.findLastIndex !== "function") {
    Array.prototype.findLastIndex = function (predicate, thisArg) {
      for (var i = this.length - 1; i >= 0; i--) {
        if (i in this && predicate.call(thisArg, this[i], i, this)) return i;
      }
      return -1;
    };
  }
  if (typeof Array.prototype.toSorted !== "function") {
    Array.prototype.toSorted = function (compare) { return this.slice().sort(compare); };
  }
  if (typeof Array.prototype.toReversed !== "function") {
    Array.prototype.toReversed = function () { return this.slice().reverse(); };
  }
  if (typeof Array.prototype.toSpliced !== "function") {
    Array.prototype.toSpliced = function (start, deleteCount) {
      var copy = this.slice();
      var items = Array.prototype.slice.call(arguments, 2);
      copy.splice.apply(copy, [start, deleteCount].concat(items));
      return copy;
    };
  }
  if (typeof Array.prototype.with !== "function") {
    Array.prototype.with = function (index, value) {
      var copy = this.slice();
      var at = index < 0 ? copy.length + index : index;
      copy[at] = value;
      return copy;
    };
  }
  if (typeof Object.hasOwn !== "function") {
    Object.hasOwn = function (object, key) { return Object.prototype.hasOwnProperty.call(object, key); };
  }
  if (typeof Promise.withResolvers !== "function") {
    Promise.withResolvers = function () {
      var resolve, reject;
      var promise = new Promise(function (res, rej) { resolve = res; reject = rej; });
      return { promise: promise, resolve: resolve, reject: reject };
    };
  }
  if (typeof scheduler !== "object" || scheduler === null) {
    globalThis.scheduler = {
      postTask: function (callback) {
        return new Promise(function (resolve, reject) {
          setTimeout(function () {
            try { resolve(callback()); } catch (error) { reject(error); }
          }, 0);
        });
      },
      yield: function () { return new Promise(function (resolve) { setTimeout(resolve, 0); }); }
    };
  }
  if (typeof PerformanceObserver !== "function") {
    function PerformanceObserver(callback) { this._callback = callback; }
    PerformanceObserver.prototype.observe = function () {};
    PerformanceObserver.prototype.disconnect = function () {};
    PerformanceObserver.prototype.takeRecords = function () { return []; };
    PerformanceObserver.supportedEntryTypes = [
      "element", "event", "first-input", "largest-contentful-paint",
      "layout-shift", "longtask", "mark", "measure", "navigation", "paint", "resource"
    ];
    globalThis.PerformanceObserver = PerformanceObserver;
  }
  if (typeof NodeFilter !== "object" || NodeFilter === null) {
    globalThis.NodeFilter = {
      FILTER_ACCEPT: 1,
      FILTER_REJECT: 2,
      FILTER_SKIP: 3,
      SHOW_ALL: 0xffffffff,
      SHOW_ELEMENT: 1,
      SHOW_TEXT: 4,
      SHOW_COMMENT: 128,
      SHOW_DOCUMENT: 256,
      SHOW_DOCUMENT_FRAGMENT: 1024
    };
  }
  function nodeBit(node) {
    if (!node) return 0;
    if (node.nodeType === 1) return 1;
    if (node.nodeType === 3) return 4;
    if (node.nodeType === 8) return 128;
    if (node.nodeType === 9) return 256;
    if (node.nodeType === 11) return 1024;
    return 0;
  }
  function TreeWalker(root, whatToShow) {
    this.root = root;
    this.currentNode = root;
    this.whatToShow = whatToShow == null ? 0xffffffff : whatToShow;
  }
  TreeWalker.prototype.nextNode = function () {
    var node = this.currentNode;
    for (;;) {
      if (node && node.firstChild) node = node.firstChild;
      else {
        while (node && node !== this.root && !node.nextSibling) node = node.parentNode;
        if (!node || node === this.root) return null;
        node = node.nextSibling;
      }
      if ((this.whatToShow & nodeBit(node)) !== 0) {
        this.currentNode = node;
        return node;
      }
    }
  };
  document.importNode = function (node, deep) {
    if (node && typeof node.cloneNode === "function") return node.cloneNode(!!deep);
    return node;
  };
  document.adoptNode = function (node) { return node; };
  document.createTreeWalker = function (root, whatToShow) {
    return new TreeWalker(root, whatToShow);
  };
  if (!document.firstElementChild) {
    Object.defineProperty(document, "firstElementChild", {
      configurable: true,
      get: function () { return document.documentElement; }
    });
  }
})();

// Compact Intl for en and bn. Boa is built without its ICU feature, so
// Date and Number locale methods would otherwise fall through to toString.
(function () {
  var BN = "০১২৩৪৫৬৭৮৯";
  var EN_MONTHS = ["January","February","March","April","May","June","July","August","September","October","November","December"];
  var EN_MONTHS_SHORT = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
  var EN_DAYS = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
  var EN_DAYS_SHORT = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"];
  var BN_MONTHS = ["জানুয়ারি","ফেব্রুয়ারি","মার্চ","এপ্রিল","মে","জুন","জুলাই","আগস্ট","সেপ্টেম্বর","অক্টোবর","নভেম্বর","ডিসেম্বর"];
  var BN_DAYS = ["রবিবার","সোমবার","মঙ্গলবার","বুধবার","বৃহস্পতিবার","শুক্রবার","শনিবার"];

  function localeTag(locales) {
    var value = locales;
    if (Array.isArray(value)) value = value[0];
    if (value == null || value === "") value = "en";
    return String(value);
  }
  function isBn(locales) {
    return localeTag(locales).toLowerCase().indexOf("bn") === 0;
  }
  function banglaDigits(text) {
    var out = "";
    var raw = String(text);
    for (var i = 0; i < raw.length; i++) {
      var code = raw.charCodeAt(i);
      out += code >= 48 && code <= 57 ? BN.charAt(code - 48) : raw.charAt(i);
    }
    return out;
  }
  function pad(value, width) {
    var text = String(value);
    while (text.length < width) text = "0" + text;
    return text;
  }
  function show(text, locales) {
    return isBn(locales) ? banglaDigits(text) : text;
  }
  function timeZone() {
    if (typeof __mithuTimeZone === "string" && __mithuTimeZone) return __mithuTimeZone;
    return "UTC";
  }

  function DateTimeFormat(locales, options) {
    if (!(this instanceof DateTimeFormat)) return new DateTimeFormat(locales, options);
    this._locales = locales;
    this._options = options || {};
  }
  DateTimeFormat.prototype.resolvedOptions = function () {
    return {
      locale: isBn(this._locales) ? "bn" : "en",
      calendar: "gregory",
      numberingSystem: isBn(this._locales) ? "beng" : "latn",
      timeZone: this._options.timeZone || timeZone()
    };
  };
  DateTimeFormat.prototype.formatToParts = function (value) {
    var formatted = this.format(value);
    return [{ type: "literal", value: formatted }];
  };
  DateTimeFormat.prototype.format = function (value) {
    var date = value instanceof Date ? value : new Date(value == null ? Date.now() : value);
    if (isNaN(date.getTime())) return "Invalid Date";
    var options = this._options;
    var locales = this._locales;
    var bn = isBn(locales);
    var year = options.year;
    var month = options.month;
    var day = options.day;
    var weekday = options.weekday;
    var hour = options.hour;
    var minute = options.minute;
    var second = options.second;
    if (options.dateStyle || options.timeStyle) {
      if (options.dateStyle) {
        year = year || "numeric";
        month = month || (options.dateStyle === "long" || options.dateStyle === "full" ? "long" : "numeric");
        day = day || "numeric";
        if (options.dateStyle === "full") weekday = weekday || "long";
      }
      if (options.timeStyle) {
        hour = hour || "numeric";
        minute = minute || "numeric";
        if (options.timeStyle !== "short") second = second || "numeric";
      }
    } else if (!year && !month && !day && !weekday && !hour && !minute && !second) {
      year = "numeric";
      month = "numeric";
      day = "numeric";
      hour = "numeric";
      minute = "numeric";
      second = "numeric";
    }
    var parts = [];
    if (weekday) {
      var names = bn ? BN_DAYS : (weekday === "short" || weekday === "narrow" ? EN_DAYS_SHORT : EN_DAYS);
      parts.push(names[date.getDay()]);
    }
    var dateParts = [];
    if (month) {
      var monthText;
      if (month === "long") monthText = (bn ? BN_MONTHS : EN_MONTHS)[date.getMonth()];
      else if (month === "short" || month === "narrow") monthText = (bn ? BN_MONTHS : EN_MONTHS_SHORT)[date.getMonth()];
      else monthText = show(month === "2-digit" ? pad(date.getMonth() + 1, 2) : String(date.getMonth() + 1), locales);
      dateParts.push(monthText);
    }
    if (day) dateParts.push(show(day === "2-digit" ? pad(date.getDate(), 2) : String(date.getDate()), locales));
    if (year) dateParts.push(show(year === "2-digit" ? pad(date.getFullYear() % 100, 2) : String(date.getFullYear()), locales));
    if (dateParts.length) {
      var numericDate = !month || month === "numeric" || month === "2-digit";
      if (numericDate && bn && dateParts.length === 3) parts.push(dateParts[1] + "/" + dateParts[0] + "/" + dateParts[2]);
      else if (numericDate) parts.push(dateParts.join("/"));
      else parts.push(dateParts.join(" "));
    }
    if (hour || minute || second) {
      var hours = date.getHours();
      var hour12 = options.hour12 !== false && options.hourCycle !== "h23" && options.hourCycle !== "h24";
      var suffix = "";
      var shownHour = hours;
      if (hour12) {
        suffix = hours < 12 ? " AM" : " PM";
        shownHour = hours % 12;
        if (shownHour === 0) shownHour = 12;
      }
      var time = [];
      if (hour) time.push(show(hour === "2-digit" ? pad(shownHour, 2) : String(shownHour), locales));
      if (minute) time.push(show(minute === "2-digit" || hour ? pad(date.getMinutes(), 2) : String(date.getMinutes()), locales));
      if (second) time.push(show(second === "2-digit" || minute ? pad(date.getSeconds(), 2) : String(date.getSeconds()), locales));
      parts.push(time.join(":") + (bn ? banglaDigits(suffix) : suffix));
    }
    return parts.join(", ");
  };
  DateTimeFormat.supportedLocalesOf = function () { return ["en", "bn"]; };

  function groupDigits(text) {
    var sign = "";
    if (text.charAt(0) === "-") {
      sign = "-";
      text = text.slice(1);
    }
    var dot = text.indexOf(".");
    var whole = dot < 0 ? text : text.slice(0, dot);
    var frac = dot < 0 ? "" : text.slice(dot);
    var grouped = "";
    for (var i = 0; i < whole.length; i++) {
      if (i > 0 && (whole.length - i) % 3 === 0) grouped += ",";
      grouped += whole.charAt(i);
    }
    return sign + grouped + frac;
  }
  function NumberFormat(locales, options) {
    if (!(this instanceof NumberFormat)) return new NumberFormat(locales, options);
    this._locales = locales;
    this._options = options || {};
  }
  NumberFormat.prototype.resolvedOptions = function () {
    return {
      locale: isBn(this._locales) ? "bn" : "en",
      numberingSystem: isBn(this._locales) ? "beng" : "latn",
      style: this._options.style || "decimal"
    };
  };
  NumberFormat.prototype.format = function (value) {
    var options = this._options;
    var number = Number(value);
    if (!isFinite(number)) return String(number);
    var style = options.style || "decimal";
    if (style === "percent") number *= 100;
    var max = options.maximumFractionDigits;
    var min = options.minimumFractionDigits;
    if (style === "currency" && max == null && min == null) {
      min = 2;
      max = 2;
    }
    if (max == null) max = style === "percent" ? 0 : 3;
    var text = max === 0 ? String(Math.round(number)) : number.toFixed(Math.min(max, 20));
    if (min != null) {
      var dot = text.indexOf(".");
      var frac = dot < 0 ? "" : text.slice(dot + 1);
      while (frac.length < min) frac += "0";
      text = (dot < 0 ? text : text.slice(0, dot)) + (frac ? "." + frac : "");
    } else if (text.indexOf(".") >= 0) {
      text = text.replace(/\.?0+$/, "");
    }
    if (options.useGrouping !== false) text = groupDigits(text);
    text = show(text, this._locales);
    if (style === "percent") text += "%";
    if (style === "currency") {
      var currency = options.currency || "USD";
      var symbol = currency === "USD" ? "$" : currency === "BDT" ? "৳" : currency + " ";
      text = symbol + text;
    }
    return text;
  };
  NumberFormat.supportedLocalesOf = function () { return ["en", "bn"]; };

  function PluralRules(locales) {
    if (!(this instanceof PluralRules)) return new PluralRules(locales);
    this._locales = locales;
  }
  PluralRules.prototype.select = function (value) {
    return Math.abs(Number(value)) === 1 ? "one" : "other";
  };
  PluralRules.prototype.resolvedOptions = function () {
    return { locale: isBn(this._locales) ? "bn" : "en", type: "cardinal" };
  };
  PluralRules.supportedLocalesOf = function () { return ["en", "bn"]; };

  var RELATIVE = {
    en: { second: ["second", "seconds"], minute: ["minute", "minutes"], hour: ["hour", "hours"], day: ["day", "days"], week: ["week", "weeks"], month: ["month", "months"], year: ["year", "years"] },
    bn: { second: ["সেকেন্ড", "সেকেন্ড"], minute: ["মিনিট", "মিনিট"], hour: ["ঘণ্টা", "ঘণ্টা"], day: ["দিন", "দিন"], week: ["সপ্তাহ", "সপ্তাহ"], month: ["মাস", "মাস"], year: ["বছর", "বছর"] }
  };
  function RelativeTimeFormat(locales) {
    if (!(this instanceof RelativeTimeFormat)) return new RelativeTimeFormat(locales);
    this._locales = locales;
  }
  RelativeTimeFormat.prototype.format = function (value, unit) {
    var number = Number(value);
    var table = isBn(this._locales) ? RELATIVE.bn : RELATIVE.en;
    var names = table[unit] || [unit, unit];
    var word = Math.abs(number) === 1 ? names[0] : names[1];
    var amount = show(String(Math.abs(number)), this._locales);
    if (isBn(this._locales)) return amount + " " + word + (number < 0 ? " আগে" : " পরে");
    return number < 0 ? amount + " " + word + " ago" : "in " + amount + " " + word;
  };
  RelativeTimeFormat.prototype.resolvedOptions = function () {
    return { locale: isBn(this._locales) ? "bn" : "en", numeric: "always", style: "long" };
  };
  RelativeTimeFormat.supportedLocalesOf = function () { return ["en", "bn"]; };

  function Collator(locales, options) {
    if (!(this instanceof Collator)) return new Collator(locales, options);
    this._options = options || {};
  }
  Collator.prototype.compare = function (left, right) {
    var a = String(left);
    var b = String(right);
    var sensitivity = this._options.sensitivity || "variant";
    if (sensitivity === "base" || sensitivity === "accent") {
      a = a.toLowerCase();
      b = b.toLowerCase();
    }
    if (a < b) return -1;
    if (a > b) return 1;
    return 0;
  };
  Collator.prototype.resolvedOptions = function () {
    return { locale: "en", sensitivity: this._options.sensitivity || "variant", collation: "default" };
  };
  Collator.supportedLocalesOf = function () { return ["en", "bn"]; };

  function Segmenter(locales, options) {
    if (!(this instanceof Segmenter)) return new Segmenter(locales, options);
    this._granularity = options && options.granularity || "grapheme";
  }
  Segmenter.prototype.segment = function (input) {
    var text = String(input == null ? "" : input);
    var parts = [];
    if (this._granularity === "word" || this._granularity === "sentence") {
      var pattern = this._granularity === "sentence" ? /[^.!?]+[.!?]*|\s+/g : /\s+|\S+/g;
      var match;
      while ((match = pattern.exec(text))) {
        parts.push({
          segment: match[0],
          index: match.index,
          input: text,
          isWordLike: /\S/.test(match[0])
        });
      }
    } else {
      for (var i = 0; i < text.length; i++) {
        parts.push({ segment: text.charAt(i), index: i, input: text });
      }
    }
    parts.containing = function (index) {
      for (var n = 0; n < parts.length; n++) {
        var part = parts[n];
        if (index >= part.index && index < part.index + part.segment.length) return part;
      }
      return undefined;
    };
    return parts;
  };
  Segmenter.prototype.resolvedOptions = function () {
    return { locale: "en", granularity: this._granularity };
  };
  Segmenter.supportedLocalesOf = function () { return ["en", "bn"]; };

  globalThis.Intl = {
    DateTimeFormat: DateTimeFormat,
    NumberFormat: NumberFormat,
    PluralRules: PluralRules,
    RelativeTimeFormat: RelativeTimeFormat,
    Collator: Collator,
    Segmenter: Segmenter
  };
  Date.prototype.toLocaleString = function (locales, options) {
    return new DateTimeFormat(locales, options).format(this);
  };
  Date.prototype.toLocaleDateString = function (locales, options) {
    var fields = options ? Object.assign({}, options) : {};
    if (!fields.dateStyle && !fields.year && !fields.month && !fields.day) fields.dateStyle = "short";
    delete fields.hour;
    delete fields.minute;
    delete fields.second;
    return new DateTimeFormat(locales, fields).format(this);
  };
  Date.prototype.toLocaleTimeString = function (locales, options) {
    var fields = options ? Object.assign({}, options) : {};
    if (!fields.timeStyle && !fields.hour && !fields.minute && !fields.second) fields.timeStyle = "medium";
    delete fields.year;
    delete fields.month;
    delete fields.day;
    delete fields.weekday;
    return new DateTimeFormat(locales, fields).format(this);
  };
  Number.prototype.toLocaleString = function (locales, options) {
    return new NumberFormat(locales, options).format(Number(this));
  };
  String.prototype.localeCompare = function (other, locales, options) {
    return new Collator(locales, options).compare(String(this), String(other));
  };
})();
