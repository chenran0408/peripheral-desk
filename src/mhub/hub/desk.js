// Shell for the locally hosted MCHOSE driver. Device-page state belongs to
// the original editor; this file only changes navigation and the connect page.
(() => {
  "use strict";

  const brand = "外设盒";
  const product = "迈从驱动中心";
  const blockedRoutes = new Set(["Advertisement", "SettingFeedback"]);
  const serviceSegments = new Set(["cloud", "mall", "promotion", "user", "logo"]);
  const serviceNodes = [
    ".header-cloud-action", ".header-mall-action", ".header-promotion",
    ".header-user-action", ".header-login-trigger", ".header-logo-action",
    ".download-client-popover", ".device-card-upgrade",
    '[data-home-card-key="advertisement-card"]', ".mouse-upgrade-panel"
  ].join(",");
  const settingsButtons = "du-button,button,[role=menuitem],a[href]";
  const segments = "du-segmented-button.app-header__action-group";
  const hidden = "data-pd-hub-hidden";
  const title = brand + " · " + product;
  let router;
  let translations;
  let strings;

  // Magnetic keyboard pages ask the parent for cloud/account services. Keep the
  // device, theme, preview and local-profile messages on their original routes.
  window.addEventListener("message", event => {
    const data = event.data;
    if (event.origin !== location.origin || typeof data?.type !== "string") return;
    if (!(data.type === "COMMAND_OPEN_LOGIN" || data.type === "PROXY_OPEN_CLOUD_LIST" ||
          data.type.startsWith("PROXY_CLOUD_ACTION_"))) return;
    const keyboard = Array.from(document.querySelectorAll("iframe")).some(frame =>
      frame.contentWindow === event.source &&
      ["/cizhou/index.html","/cizhou/preview/index.html"].includes(new URL(frame.src, location.href).pathname));
    if (!keyboard) return;
    event.stopImmediatePropagation();
    if (data._messageId) event.source.postMessage({
      commandType:data.type, _messageId:data._messageId, err:"本地驱动中心仅使用本地配置文件。"
    }, location.origin);
  }, true);

  function words() {
    const app = document.querySelector("#app")?.__vue_app__;
    const i18n = app?._context.provides[app.__VUE_I18N_SYMBOL__]?.global;
    const language = i18n?.locale?.value ?? i18n?.locale;
    if (strings && translations === language) return strings;
    const labels = keys => {
      const values = new Set();
      for (const key of keys) {
        if (!i18n?.t || (i18n.te && !i18n.te(key))) continue;
        const value = i18n.t(key);
        if (typeof value === "string" && value !== key) values.add(value.trim());
      }
      return values;
    };
    strings = {
      services: labels(["menu.cloud", "mouse.shareToCloud", "common.cloudImports",
        "setting.viewUpdateHistory", "setting.autoUpdate", "setting.autoUpdateSilent"]),
      updates: labels(["mouse.upgradeNowBtn", "mouse.alreadyUpDate", "mouse.continueUpgrade",
        "mouse.firmwareUpgrade", "mouse.upgradeText", "connectDevice.importantUpdate",
        "connectDevice.upgradable", "connectDevice.upgradeErrorRetry"]),
      version: labels(["mouse.mouseFirmwareVersion", "mouse.receiverFirmwareVersion",
        "mouse.magDockFirmwareVersion", "keyboard.keyboardFirmwareVersion"])
    };
    translations = language;
    return strings;
  }

  function addBrand(header) {
    if (header.querySelector(".pd-hub-brand")) return;
    const text = document.createElement("span");
    text.className = "pd-hub-brand";
    text.append(brand);
    const detail = document.createElement("small");
    detail.textContent = product;
    text.append(detail);
    // Other headings provide a back button and the selected device's name.
    if (header.querySelector(".app-header__logo-group")) header.prepend(text);
  }

  function welcome(screen) {
    if (screen.querySelector(".pd-hub-help")) return;
    const heading = screen.querySelector(".web-no-device__title");
    const instructions = screen.querySelector(".web-no-device__description");
    if (!heading || !instructions || !screen.querySelector(".web-no-device__authorize")) return;
    heading.textContent = "连接迈从设备";
    instructions.textContent = "点击「连接设备」，在弹出的列表里选中要设置的键盘、鼠标或其他外设。";
    const help = document.createElement("dl");
    help.className = "pd-hub-help";
    help.setAttribute("aria-label", "连接方式");
    for (const [name, text] of [
      ["2.4 GHz 接收器", "插入接收器，将设备切换到无线接收器模式。"],
      ["USB 数据线", "用支持数据传输的线连接设备；仅供电的线无法识别。"]
    ]) {
      const key = document.createElement("dt");
      const value = document.createElement("dd");
      key.textContent = name;
      value.textContent = text;
      help.append(key, value);
    }
    screen.append(help);
  }

  // The real clickable top-bar segments live inside the design system's
  // shadow root. Slotted text alone doesn't own focus or click handling.
  function prepareSegments(group) {
    const slots = [...group.children].filter(child => child.hasAttribute("slot"));
    if (slots.length) group.toggleAttribute(hidden, slots.every(node => node.matches(serviceNodes)));
    const root = group.shadowRoot;
    if (!root) return;
    for (const button of root.querySelectorAll("button[data-value]")) {
      if (serviceSegments.has(button.dataset.value)) {
        button.dataset.pdHubService = "1";
        button.inert = true;
        button.style.setProperty("display", "none", "important");
      } else if (button.hasAttribute("data-pd-hub-service")) {
        delete button.dataset.pdHubService;
        button.inert = false;
        button.style.removeProperty("display");
      }
    }
    if (!group.dataset.pdHubSegments) {
      group.dataset.pdHubSegments = "1";
      new MutationObserver(() => prepareSegments(group)).observe(root, {childList:true, subtree:true});
    }
  }

  function translatedAction(node) {
    const dict = words();
    const label = node.textContent?.trim() || node.getAttribute("aria-label") || "";
    if (dict.services.has(label)) return true;
    // “固件配置” on M HUB can mean an onboard profile. Only update buttons
    // in the actual firmware/other-settings UI are restricted.
    if (!dict.updates.has(label)) return false;
    if (node.closest(".other-setting,.guide-dialog,.mouse-upgrade-panel,.device-card-upgrade")) return true;
    let row = node.parentElement;
    for (let level = 0; row && level < 4; level++, row = row.parentElement) {
      const description = [row.getAttribute("title"), row.textContent].filter(Boolean).join(" ");
      if ([...dict.version].some(version => description.includes(version))) return true;
    }
    return false;
  }

  function localRoutes() {
    const next = document.querySelector("#app")?.__vue_app__?.config.globalProperties.$router;
    if (!next || next === router) return;
    router = next;
    next.beforeEach((to, from) => {
      if (blockedRoutes.has(to.name)) return from.name && !blockedRoutes.has(from.name) ? false : {name:"ConnectDevice"};
    });
    if (blockedRoutes.has(next.currentRoute.value.name)) next.replace({name:"ConnectDevice"});
  }

  function visit(scope, selector, fn) {
    if (scope.matches?.(selector)) fn(scope);
    for (const elem of scope.querySelectorAll(selector)) fn(elem);
  }

  function scan(scope) {
    localRoutes();
    visit(scope, ".app-header__left", addBrand);
    visit(scope, ".web-no-device", welcome);
    visit(scope, segments, prepareSegments);
    visit(scope, serviceNodes, elem => elem.setAttribute(hidden, ""));
    visit(scope, settingsButtons, elem => {
      if (translatedAction(elem)) elem.setAttribute(hidden, "");
      else if (!elem.matches(serviceNodes)) elem.removeAttribute(hidden);
    });
  }

  function install() {
    if (document.title !== title) document.title = title;
    const queue = new Set();
    let scheduled = false;
    const flush = () => {
      scheduled = false;
      for (const el of queue) {
        if (!el.isConnected) continue;
        let parent = el.parentElement;
        while (parent && !queue.has(parent)) parent = parent.parentElement;
        if (!parent) scan(el);
      }
      queue.clear();
    };
    new MutationObserver(records => {
      for (const record of records) {
        const elem = record.target.nodeType === Node.ELEMENT_NODE ? record.target : record.target.parentElement;
        if (!elem) continue;
        const surface = elem.closest([segments,settingsButtons,".app-header__left",".web-no-device"].join(","));
        if (surface) queue.add(surface);
        for (const item of record.addedNodes) if (item.nodeType === Node.ELEMENT_NODE) queue.add(item);
      }
      if (queue.size && !scheduled) { scheduled = true; requestAnimationFrame(flush); }
    }).observe(document.body, {subtree:true, childList:true, characterData:true});
    // Block activation during the frame in which Vue replaces a menu item.
    document.addEventListener("click", event => {
      if (event.composedPath().some(node => node instanceof Element &&
          (node.matches(serviceNodes) || node.hasAttribute(hidden) ||
          (node.matches(settingsButtons) && translatedAction(node)) ||
          (node.getRootNode().host?.matches(segments) && serviceSegments.has(node.getAttribute("data-value")))))) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    }, true);
    scan(document.body);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", install, {once:true});
  else install();
})();
