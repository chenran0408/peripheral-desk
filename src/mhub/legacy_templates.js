// Read only the current model's M HUB library at the app's own origin.
async ({ product, firstGeneration, keyboard }) => {
  const normalize = value => String(value).trim()
    .replace(/^(?:RealTek|YJX-CHIP)\s*/i, "")
    .replace(/^(?:迈从|MCHOSE)\s+/i, "").toLowerCase();
  const expected = normalize(product);
  const matches = value => {
    const actual = normalize(value);
    return actual === expected || (expected === "a7 v3 magnetic" && actual === "a7 magnetic");
  };
  const request = req => new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
    req.onblocked = () => reject(new Error("旧模板库正在由其他窗口升级，请稍后再试。"));
  });
  const databases = await indexedDB.databases();
  const existing = async name => {
    if (!databases.some(db => db.name === name)) return null;
    const open = indexedDB.open(name);
    open.onupgradeneeded = () => open.transaction.abort();
    return request(open);
  };
  const rows = [];
  if (keyboard.length) {
    for (const { type, ids } of keyboard) {
      const db = await existing("cizhou@type-" + type);
      if (!db) continue;
      try {
        // localForage replaces punctuation in the vendor's "local-data@v2" store name.
        if (!db.objectStoreNames.contains("local_data_v2")) continue;
        const store = db.transaction("local_data_v2", "readonly").objectStore("local_data_v2");
        const [profiles, macros, onboardNames] = await Promise.all(
          ["profiles", "__macros__", "__profileNames__"].map(key => request(store.get(key))));
        for (const data of profiles || []) {
          const id = data?.extra?.identity;
          if (!ids.some(([vid, pid]) => Number(id?.vendorId) === vid && Number(id?.productId) === pid) || !data.name) continue;
          const type = Array.isArray(onboardNames) && onboardNames.includes(data.name) ? "keyboard" : "localstorage";
          rows.push({dataScope: "KeyboardProfile", version: 3, type,
            vendorId: id.vendorId, productId: id.productId, productName: id.productName,
            identity: id, data, ...(macros ? {macros} : {})});
        }
      } finally { db.close(); }
    }
    return rows;
  }
  const wrap = (store, entry, slot) => {
    if (!entry || typeof entry !== "object") return;
    const label = store.trim().replace(/^(?:RealTek|YJX-CHIP)\s*/i, "");
    const shape = firstGeneration ? Boolean(entry.allInfo) :
      ["leftKeyType", "dpiSum", "usbRateIndex", "sleep", "sensor", "lodIndex", "adaptivePowerSwitch"].some(k => Object.hasOwn(entry, k));
    if (!shape) return;
    const record = {...entry};
    // Same fields discarded by M HUB's export-to-file operation.
    for (const key of ["uid", "cid", "id", "updateTime", "orderNum", "source"]) delete record[key];
    if (typeof record.name !== "string" || !record.name.trim()) {
      if (slot === undefined) return;
      record.name = "板载配置 " + (slot + 1);
    }
    rows.push({[label]: record});
  };
  const mouse = JSON.parse(localStorage.getItem("mouse") || "{}");
  for (const [model, data] of Object.entries(mouse)) {
    if (matches(model) && Array.isArray(data?.templateList)) {
      data.templateList.forEach((entry, slot) => wrap(model, entry, slot));
    }
  }
  for (const library of ["mouse-custom", "mouse-cloud"]) {
    const db = await existing(library);
    if (!db) continue;
    try {
      for (const name of db.objectStoreNames) {
        if (!matches(name)) continue;
        const all = await request(db.transaction(name, "readonly").objectStore(name).getAll());
        for (const entry of all) wrap(name, entry);
      }
    } finally { db.close(); }
  }
  return rows;
}
