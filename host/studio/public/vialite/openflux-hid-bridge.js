/* OpenFlux Studio -> Vialite WebHID bridge (routes 32-byte Vial packets through openfluxd). */
(function () {
  if (window.parent === window) return;

  /* Inside sandbox="allow-scripts allow-downloads" (opaque origin), accessing window.localStorage
   * throws SecurityError. Provide a clean in-memory Storage shim so Vialite runs seamlessly
   * while remaining 100% isolated from Studio's origin and Tauri IPC. */
  try {
    void window.localStorage;
  } catch (_) {
    var mem = Object.create(null);
    var memStorage = {
      getItem: function (k) {
        return Object.prototype.hasOwnProperty.call(mem, String(k)) ? mem[String(k)] : null;
      },
      setItem: function (k, v) {
        mem[String(k)] = String(v);
      },
      removeItem: function (k) {
        delete mem[String(k)];
      },
      clear: function () {
        mem = Object.create(null);
      },
      key: function (i) {
        return Object.keys(mem)[i] || null;
      },
      get length() {
        return Object.keys(mem).length;
      },
    };
    try {
      Object.defineProperty(window, "localStorage", { value: memStorage, configurable: true });
    } catch (__) {}
  }

  try {
    if (!localStorage.getItem("vialite.lang") && !localStorage.getItem("lang")) {
      localStorage.setItem("vialite.lang", "en");
    }
  } catch (_) {}

  var nextReqId = 1;
  var pending = new Map();

  function rpc(type, payload) {
    return new Promise(function (resolve, reject) {
      var reqId = nextReqId++;
      var timer = setTimeout(function () {
        pending.delete(reqId);
        reject(new Error("OpenFlux Vial bridge timed out"));
      }, 5000);
      pending.set(reqId, { resolve: resolve, reject: reject, timer: timer });
      window.parent.postMessage(Object.assign({ type: type, reqId: reqId }, payload || {}), "*");
    });
  }

  function OpenFluxHidDevice(info) {
    this._target = new EventTarget();
    this.vendorId = info.vendor_id || 0x3761;
    this.productId = info.product_id || 0x4015;
    this.productName = info.product_name || "OpenFlux Keyboard";
    this.opened = false;
    this.collections = [{ usagePage: 0xff60, usage: 0x61 }];
  }
  OpenFluxHidDevice.prototype.addEventListener = function (t, fn, opt) {
    this._target.addEventListener(t, fn, opt);
  };
  OpenFluxHidDevice.prototype.removeEventListener = function (t, fn, opt) {
    this._target.removeEventListener(t, fn, opt);
  };
  OpenFluxHidDevice.prototype.dispatchEvent = function (ev) {
    return this._target.dispatchEvent(ev);
  };
  OpenFluxHidDevice.prototype.open = function () {
    this.opened = true;
    return Promise.resolve();
  };
  OpenFluxHidDevice.prototype.close = function () {
    this.opened = false;
    return Promise.resolve();
  };
  OpenFluxHidDevice.prototype.forget = function () {
    this.opened = false;
    return Promise.resolve();
  };
  OpenFluxHidDevice.prototype.sendReport = function (_reportId, data) {
    var self = this;
    var u8 =
      data instanceof Uint8Array
        ? data
        : ArrayBuffer.isView(data)
          ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
          : new Uint8Array(data);
    return rpc("openflux:vial:xfer", { data: Array.from(u8) }).then(function (res) {
      var reply = new Uint8Array(32);
      if (res && Array.isArray(res.data)) {
        for (var i = 0; i < Math.min(32, res.data.length); i++) reply[i] = res.data[i] & 0xff;
      }
      var ev = new Event("inputreport");
      ev.device = self;
      ev.reportId = 0;
      ev.data = new DataView(reply.buffer);
      self._target.dispatchEvent(ev);
    });
  };

  var hidTarget = new EventTarget();
  var cachedDevice = null;

  function fetchDevices() {
    return rpc("openflux:vial:status").then(function (st) {
      if (!st || !st.connected) {
        cachedDevice = null;
        return [];
      }
      if (!cachedDevice) cachedDevice = new OpenFluxHidDevice(st);
      return [cachedDevice];
    });
  }

  var openfluxHid = {
    getDevices: function () {
      return fetchDevices();
    },
    requestDevice: function () {
      return fetchDevices();
    },
    addEventListener: function (t, fn, opt) {
      hidTarget.addEventListener(t, fn, opt);
    },
    removeEventListener: function (t, fn, opt) {
      hidTarget.removeEventListener(t, fn, opt);
    },
  };

  window.addEventListener("message", function (ev) {
    var msg = ev.data;
    if (!msg || typeof msg !== "object") return;
    if (msg.type === "openflux:vial:res" && pending.has(msg.reqId)) {
      var entry = pending.get(msg.reqId);
      pending.delete(msg.reqId);
      clearTimeout(entry.timer);
      if (msg.error) entry.reject(new Error(String(msg.error)));
      else entry.resolve(msg.result);
    } else if (msg.type === "openflux:vial:disconnect" && cachedDevice) {
      var dev = cachedDevice;
      cachedDevice = null;
      var dis = new Event("disconnect");
      dis.device = dev;
      hidTarget.dispatchEvent(dis);
    }
  });

  // Define on both Navigator.prototype and navigator so WKWebView ("hid" in navigator) succeeds.
  try {
    if (typeof Navigator !== "undefined" && Navigator.prototype) {
      Object.defineProperty(Navigator.prototype, "hid", {
        get: function () {
          return openfluxHid;
        },
        configurable: true,
      });
    }
  } catch (_) {}
  try {
    Object.defineProperty(navigator, "hid", {
      value: openfluxHid,
      configurable: true,
    });
  } catch (_) {
    try {
      navigator.hid = openfluxHid;
    } catch (_) {}
  }
})();
