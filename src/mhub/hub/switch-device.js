// The native toolbar uses this top-level entry in every local edition,
// including previously saved editions with a custom device list.
(() => {
  const hub = window.PeripheralDeskHub ||= {};
  function home() {
    const router = document.querySelector("#app")?.__vue_app__?.config.globalProperties.$router;
    if (router) return router.push({name:"ConnectDevice"});
    location.hash = "/connectDevice";
  }
  hub.chooseDevice ||= () => {
    hub.stayInChooser = true;
    return home();
  };
  hub.openModel ||= model => {
    window.__peripheralDeskPreferredModel = model || "";
    hub.stayInChooser = false;
    return home();
  };
})();
