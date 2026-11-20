// Startup shortcut for the local M HUB. The official device cards and their
// actual entry action still own identification, routing and editor lifecycle.
function createPeripheralConnection({ref, watch, visible, familiesReady, cards, enter}) {
  const session = window.PeripheralDeskHub ||= {};
  const completed = ref(false);
  let scan;

  function consider() {
    if (session.stayInChooser || !visible() || !completed.value || !familiesReady()) return;
    const current = cards();
    if (current.count > 1) session.stayInChooser = true;
    if (current.count !== 1 || !current.single) return;
    // Set this before calling the real entry action so a return, disconnect
    // or cancelled vendor flow never opens the same device again by itself.
    session.stayInChooser = true;
    enter(current.single);
  }

  watch(() => [visible(), completed.value, familiesReady(), cards()], consider, {flush:"post"});
  return {
    begin(token) { scan = token; completed.value = false; },
    finish(token, ok) { if (scan === token) completed.value = ok; }
  };
}
