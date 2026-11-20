// Local integration for the magnetic keyboard editor and read-only profile preview.
(() => {
  "use strict";
  const hidden = "data-pd-local-hidden";
  const modes = ".config-import-dialog__modes";
  const fwAction = "[data-upgrade-action]";
  const header = ".text-du-typo-headline-semibold-l";
  // These are the nine bundled translations of the firmware-download row (3005).
  const packageTitles = new Set([
    "固件升级包下载", "Download the firmware upgrade package",
    "Descargar el paquete de actualización del firmware",
    "Télécharger le paquet de mise à niveau du logiciel de base",
    "Firmware-Upgradepaket herunterladen", "ファームウェアアップグレードパッケージをダウンロード",
    "펌웨어 업그레이드 패키지 다운로드", "Pobierz pakiet uaktualnienia firmware",
    "Скачать пакет обновления прошивки"
  ]);

  function showFileImport(group) {
    if (group.hasAttribute(hidden) || !group.querySelector('[data-value="file"]')) return;
    group.dispatchEvent(new CustomEvent("change", {
      detail: {value:"file"}, bubbles:true, composed:true
    }));
    group.setAttribute(hidden, "");
  }

  function firmwareExit(action) {
    const footer = action.parentElement;
    if (!footer || footer.querySelector(".pd-fw-exit")) return;
    const row = document.createElement("div");
    row.className = "pd-fw-exit";
    const detail = document.createElement("span");
    detail.textContent = "当前键盘处于固件升级流程。退出升级模式后可重新连接。";
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = "返回设备列表";
    button.addEventListener("click", () => {
      window.parent.postMessage({type:"KEYBOARD_GO_BACK_LIST_PAGE", firmwareUpgradeSuccess:false}, location.origin);
    });
    row.append(detail, button);
    footer.append(row);
    button.focus({preventScroll:true});
  }

  function scan(element) {
    const selector = [modes, fwAction, header].join(",");
    const update = node => {
      if (node.matches(modes)) showFileImport(node);
      else if (node.matches(fwAction)) firmwareExit(node);
      else {
        // Title and action are siblings inside the dedicated settings card.
        const controls = node.parentElement?.parentElement;
        const card = controls?.parentElement;
        if (card && controls.querySelector('[slot="end"]') && card.classList.contains("m-auto")) {
          card.toggleAttribute(hidden, packageTitles.has(node.textContent.trim()));
        }
      }
    };
    if (element.matches(selector)) update(element);
    for (const node of element.querySelectorAll(selector)) update(node);
  }
  function mount() {
    const observer = new MutationObserver(records => {
      for (const record of records) {
        const target = record.target.nodeType === Node.ELEMENT_NODE ? record.target : record.target.parentElement;
        if (target?.matches(header + "," + modes)) scan(target);
        for (const node of record.addedNodes) if (node.nodeType === Node.ELEMENT_NODE) scan(node);
      }
    });
    observer.observe(document.body, {childList:true, characterData:true, subtree:true});
    scan(document.body);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mount, {once:true});
  else mount();
})();
