pdConnection=createPeripheralConnection({
  ref, watch,
  visible: () => Ua.value,
  familiesReady: () => sa.initialized,
  cards: () => {
    const countById = new Map();
    for (const record of [...ga.value, ...Ma.value, ...Ca.value]) {
      const id = getHomeDeviceRuntimeKey(record);
      countById.set(id, (countById.get(id) || 0) + 1);
    }
    return Pa.value.flatMap(card => {
      const runtime = getHomeDeviceRuntimeKey(card);
      if (!runtime || countById.get(runtime) !== 1 || card.source === "receiver" ||
          ["pending","failed"].includes(card.identificationState) || Ai(card) !== "normal") return [];
      const audio = ui(card);
      const key = audio ? card.modelKey : card.productKey;
      if (!key || (audio && !card.logicalDeviceId)) return [];
      return [{card, runtime, key, kind:audio ? "audio" : getConnectionKind(card),
        names:[card.deviceName, card.productName, key].filter(Boolean)}];
    });
  },
  enter: os,
  home: () => $a.push({name:"ConnectDevice"})
})
