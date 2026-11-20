// The native toolbar uses this top-level entry in every local edition,
// including previously saved editions with a custom device list.
(() => {
  const hub = window.PeripheralDeskHub ||= {};
  hub.chooseDevice ||= () => {
    hub.stayInChooser = true;
    const router = document.querySelector("#app")?.__vue_app__?.config.globalProperties.$router;
    if (router) return router.push({name:"ConnectDevice"});
    location.hash = "/connectDevice";
  };
})();
