// WebHID-shaped frontend for the built-in local editor's Windows device provider.
// The host serves this resource only to its reviewed frontend and checks every request.
(() => {
  "use strict";

  const origin = "https://mhub.peripheraldesk.test";
  if (location.origin !== origin) return;
  const installed = Symbol.for("PeripheralDesk.localHid");
  if (navigator.hid?.[installed]) return;
  function install(provider) {
    Object.defineProperty(navigator, "hid", {configurable:true, value:provider});
  }

  if (window !== window.top) {
    try {
      const top = window.top;
      if (top.location.origin === origin && top.navigator.hid?.[installed]) install(top.navigator.hid);
    } catch { /* A cross-origin embed has no local transport. */ }
    return;
  }

  const bus = window.chrome?.webview;
  if (!bus) return;
  const browser = navigator.hid;
  const channel = "peripheral-desk-hid";
  const pending = new Map();
  const devices = new Map();
  const browserAliases = new WeakMap();
  const announced = new WeakSet();
  let currentNative = [];
  let nextId = 0;
  let listing;

  function request(op, args = {}) {
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      pending.set(id, {resolve, reject});
      try { bus.postMessage({channel, id, op, ...args}); }
      catch (error) { pending.delete(id); reject(error); }
    });
  }

  function reportNumber(value) {
    // Several official command tables use hexadecimal strings for their report IDs.
    const id = Math.trunc(Number(value));
    if (!Number.isFinite(id) || id < 0 || id > 255) throw new TypeError("无效的 HID 报告编号");
    return id;
  }

  function bytes(source) {
    // DataView and typed-array slices may refer to just part of a backing buffer.
    if (ArrayBuffer.isView(source)) return Array.from(new Uint8Array(source.buffer, source.byteOffset, source.byteLength));
    if (Object.prototype.toString.call(source) === "[object ArrayBuffer]") return Array.from(new Uint8Array(source));
    throw new TypeError("HID 报告需要字节缓冲区");
  }

  function eventHandler(prototype, type) {
    const handlers = new WeakMap();
    Object.defineProperty(prototype, "on" + type, {
      configurable:true, enumerable:true,
      get() { return handlers.get(this) ?? null; },
      set(fn) {
        const previous = handlers.get(this);
        if (previous) this.removeEventListener(type, previous);
        if (typeof fn === "function") {
          handlers.set(this, fn);
          this.addEventListener(type, fn);
        } else handlers.delete(this);
      }
    });
  }

  function emit(target, type, fields) {
    target.dispatchEvent(Object.assign(new Event(type), fields));
  }

  class Device extends EventTarget {
    #handle;
    #opened = false;
    #connected = true;
    #transition;
    constructor(descriptor) {
      super();
      // M HUB attaches its own id, color and model metadata to this object.
      this.#handle = descriptor.id;
      this.vendorId = descriptor.vendorId;
      this.productId = descriptor.productId;
      this.productName = descriptor.productName;
      this.collections = descriptor.collections;
    }
    get [Symbol.toStringTag]() { return "HIDDevice"; }
    get opened() { return this.#opened; }

    async #setOpen(open) {
      if (this.#transition) {
        await this.#transition;
        return this.#setOpen(open);
      }
      if (!this.#connected) throw new DOMException("设备已断开", "InvalidStateError");
      if (open === this.#opened) return;
      const changing = request(open ? "open" : "close", {device:this.#handle}).then(() => {
        if (!this.#connected) throw new DOMException("设备已断开", "InvalidStateError");
        this.#opened = open;
      });
      this.#transition = changing;
      try { await changing; }
      finally { if (this.#transition === changing) this.#transition = undefined; }
    }
    open() { return this.#setOpen(true); }
    close() { return this.#setOpen(false); }
    async #transfer(op, reportId, data) {
      if (!this.#opened) throw new DOMException("设备尚未打开", "InvalidStateError");
      const args = {device:this.#handle, reportId:reportNumber(reportId)};
      if (op !== "receiveFeature") args.data = bytes(data);
      return request(op, args);
    }
    async sendReport(reportId, data) { await this.#transfer("sendOutput", reportId, data); }
    async sendFeatureReport(reportId, data) { await this.#transfer("sendFeature", reportId, data); }
    async receiveFeatureReport(reportId) {
      // Windows/Chromium exposes the returned first byte for a numbered feature report.
      const raw = await this.#transfer("receiveFeature", reportId);
      return new DataView(Uint8Array.from(raw).buffer);
    }
    disconnect() { this.#connected = this.#opened = false; }
  }
  eventHandler(Device.prototype, "inputreport");

  function remember(descriptor) {
    let device = devices.get(descriptor.id);
    if (!device) {
      device = new Device(descriptor);
      devices.set(descriptor.id, device);
    }
    return device;
  }
  function matches(device, filter) {
    if (filter.vendorId !== undefined && device.vendorId !== filter.vendorId) return false;
    if (filter.productId !== undefined && device.productId !== filter.productId) return false;
    if (filter.usagePage === undefined && filter.usage === undefined) return true;
    return device.collections.some(c =>
      (filter.usagePage === undefined || filter.usagePage === c.usagePage) &&
      (filter.usage === undefined || filter.usage === c.usage));
  }
  function nativeDevices() {
    listing ??= request("list").then(items => {
      currentNative = items.map(remember);
      return currentNative;
    }).finally(() => { listing = undefined; });
    return listing;
  }

  // Chromium may expose extra protected keyboard/consumer collections alongside
  // the vendor collections belonging to the same Windows interface.
  function identity(device) {
    const all = device.collections;
    const vendor = all.filter(c => c.usagePage >= 0xff00);
    const keys = (vendor.length ? vendor : all).map(c => c.usagePage + "/" + c.usage).sort();
    return keys.length && JSON.stringify([device.vendorId, device.productId, device.productName, keys]);
  }
  function combine(native, granted) {
    const remaining = new Set(native);
    const result = native.slice();
    const unpaired = [];
    // Retain each previous physical pairing even when the list order changes.
    for (const original of granted) {
      if (!remaining.delete(browserAliases.get(original))) unpaired.push(original);
    }
    for (const original of unpaired) {
      const key = identity(original);
      const match = key && [...remaining].find(d => identity(d) === key);
      if (match) {
        browserAliases.set(original, match);
        remaining.delete(match);
      } else {
        browserAliases.delete(original);
        result.push(original);
      }
    }
    return result;
  }
  const filtered = (items, filters, excluded) => items.filter(d =>
    (!filters.length || filters.some(f => matches(d, f))) && !excluded.some(f => matches(d, f)));
  class LocalHid extends EventTarget {
    get [installed]() { return true; }
    get [Symbol.toStringTag]() { return "HID"; }
    async getDevices() {
      const [native, granted] = await Promise.allSettled([nativeDevices(), browser?.getDevices?.() ?? []]);
      if (native.status === "fulfilled" && granted.status === "fulfilled") return combine(native.value, granted.value);
      if (native.status === "fulfilled" && native.value.length) return native.value.slice();
      if (granted.status === "fulfilled" && granted.value.length) return granted.value;
      throw native.status === "rejected" ? native.reason : granted.reason;
    }
    async requestDevice(options) {
      const {filters, exclusionFilters = []} = options;
      // A model-specific action can use an already present native device.
      // The gallery's broad "connect" action still lets the user add another
      // browser-supported model. Open that chooser before yielding activation.
      const covered = filters.length && filters.every(f => f.vendorId !== undefined &&
        f.productId !== undefined && currentNative.some(d => matches(d, f)));
      if (covered || !browser?.requestDevice) {
        return filtered(await this.getDevices(), filters, exclusionFilters);
      }
      const picked = await browser.requestDevice(options);
      if (!picked.length) return [];
      return this.getDevices().then(() => picked.map(d => browserAliases.get(d) ?? d), () => picked);
    }
  }
  eventHandler(LocalHid.prototype, "connect");
  eventHandler(LocalHid.prototype, "disconnect");
  const provider = new LocalHid();
  function connected(device) {
    if (announced.has(device)) return;
    announced.add(device);
    emit(provider, "connect", {device});
  }
  browser?.addEventListener?.("connect", ({device}) => {
    provider.getDevices().then(() => connected(browserAliases.get(device) ?? device), () => connected(device));
  });
  browser?.addEventListener?.("disconnect", ({device}) => {
    // Native notifications own the lifetime of an endpoint represented by both transports.
    if (browserAliases.has(device)) return;
    announced.delete(device);
    emit(provider, "disconnect", {device});
  });

  bus.addEventListener("message", event => {
    const message = event.data;
    if (message?.channel !== channel) return;
    if (Object.hasOwn(message, "id")) {
      const job = pending.get(message.id);
      if (!job) return;
      pending.delete(message.id);
      if (message.ok) job.resolve(message.value);
      else job.reject(new DOMException(message.error || "设备通讯失败", "NetworkError"));
      return;
    }
    if (message.op === "connect") {
      const {descriptor} = message;
      if (descriptor) {
        const device = remember(descriptor);
        if (!currentNative.includes(device)) currentNative = [...currentNative, device];
        connected(device);
      }
      return;
    }
    const device = devices.get(message.device);
    if (!device) return;
    if (message.op === "disconnect") {
      device.disconnect();
      devices.delete(message.device);
      currentNative = currentNative.filter(d => d !== device);
      announced.delete(device);
      emit(provider, "disconnect", {device});
    } else if (message.op === "input" && device.opened) {
      emit(device, "inputreport", {
        device, reportId:message.reportId, data:new DataView(Uint8Array.from(message.data).buffer)
      });
    }
  });
  install(provider);
})();
