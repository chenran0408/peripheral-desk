pdConnection=createPeripheralConnection({
  ref, watch,
  visible: () => Ua.value,
  familiesReady: () => sa.initialized,
  cards: () => {
    const originals = [...ga.value, ...Ma.value, ...Ca.value];
    const displayed = Pa.value;
    const count = Math.max(originals.length, displayed.length);
    const card = displayed.length === 1 ? displayed[0] : null;
    const identified = card && card.source !== "receiver" && (
      ui(card) ? card.logicalDeviceId && card.modelKey : card.productKey
    );
    const available = identified && Ai(card) === "normal" &&
      !["pending", "failed"].includes(card.identificationState);
    return {count, single:available ? card : null};
  },
  enter: os
})
