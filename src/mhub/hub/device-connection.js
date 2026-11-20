// Use the original card action for both automatic and manual entry. Each
// model keeps its own editor, first-use flow and connection handling.
function createPeripheralConnection({ref, watch, visible, familiesReady, cards, enter, home}) {
  const session = window.PeripheralDeskHub ||= {};
  const completed = ref(false);
  const request = ref(0);
  let scan;
  const lastKey = "peripheraldesk.mhub.lastModel";

  // A5 ultra and A5 V2 Ultra are distinct upstream names. Strip only the
  // brand and separators, retaining the generation, suffix and plus variants.
  const model = value => String(value ?? "").normalize("NFKC").trim().toLowerCase()
    .replace(/^(?:mchose|迈从)[\s_-]*/, "").replace(/\+/g,"plus").replace(/\((\d+)\)/g,"$1")
    .replace(/[\s_-]+/g, "");
  const compare = (left, right) => left < right ? -1 : left > right ? 1 : 0;

  function consider() {
    if (session.stayInChooser || !visible()) return;
    const choices = cards();
    if (!choices.length) return;
    const preferred = model(window.__peripheralDeskPreferredModel);
    let choice;
    if (preferred) {
      // An explicit physical target filters discovery to one endpoint. Its own
      // resolved card can open while unrelated families finish initialization.
      if (!window.__peripheralDeskTargetInstance && (!completed.value || !familiesReady())) return;
      const matches = choices.filter(row => row.names.some(name => model(name) === preferred));
      if (matches.length !== 1) return;
      choice = matches[0];
    } else {
      if (!completed.value || !familiesReady()) return;
      let last;
      try { last = JSON.parse(localStorage.getItem(lastKey)); } catch { /* A saved edition may edit this preference. */ }
      const ordered = choices.sort((a,b) => compare(model(a.names[0]), model(b.names[0])) ||
        compare(String(a.key),String(b.key)) || compare(a.runtime,b.runtime));
      if (last) {
        const sameKind = ordered.filter(row => row.kind === last.kind);
        const exact = sameKind.filter(row => String(row.key) === last.key);
        if (exact.length === 1) choice = exact[0];
        else {
          // A shared key may need a model name; switching transport can change the key.
          const names = (exact.length ? exact : sameKind).filter(row =>
            row.names.some(name => model(name) === last.name));
          if (names.length === 1) choice = names[0];
        }
      }
      choice ||= ordered[0];
    }
    session.stayInChooser = true;
    enter(choice.card);
  }

  // The toolbar and native list may re-use an already open WebView.
  session.openModel = async preferred => {
    window.__peripheralDeskPreferredModel = preferred || "";
    session.stayInChooser = false;
    if (!visible()) {
      completed.value = false;
      await home();
    }
    request.value++;
  };
  watch(() => [visible(), completed.value, familiesReady(), cards(), request.value],
    consider, {flush:"post"});

  return {
    begin(token) { scan = token; completed.value = false; },
    finish(token, ok) { if (scan === token) completed.value = ok; },
    arrived(to, from, failure) {
      if (failure || from.name !== "ConnectDevice") return;
      let kind, key;
      if (to.name === "Keyboard") [kind,key] = ["keyboard","lastKBProductKey"];
      else if (to.name === "Mouse" || to.name === "NewMouse") [kind,key] = ["mouse","lastMouseProductKey"];
      else if (to.matched.some(route => route.name === "AudioDetail"))
        [kind,key] = ["audio","lastAudioModelKey"];
      if (!key) return;
      const actual = localStorage.getItem(key);
      if (actual) {
        const name = model(localStorage.getItem("trackDeviceName"));
        localStorage.setItem(lastKey, JSON.stringify({kind, key:actual, name}));
        session.stayInChooser = true;
      }
    }
  };
}
