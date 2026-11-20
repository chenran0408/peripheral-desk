const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const localScript = name => fs.readFileSync(path.join(__dirname, "../hub", name), "utf8");
const keyboard = {productKey:"G75_Pro", identificationState:"resolved"};
const mouse = {productKey:"A5_Ultra"};

function screen() {
  const opened = [], routes = [];
  let notify;
  const scope = {
    window:{}, document:{querySelector:() => ({__vue_app__:{config:{globalProperties:{
      $router:{push:to => routes.push(to.name)}
    }}}})},
    ref:value => ({value}), watch:(_, callback) => { notify = callback; },
    Ua:{value:true}, sa:{initialized:true},
    ga:{value:[]}, Ma:{value:[]}, Ca:{value:[]}, Pa:{value:[]},
    ui:card => card.source === "audio", Ai:card => card.state || "normal", os:card => opened.push(card)
  };
  vm.createContext(scope);
  for (const file of ["switch-device.js", "device-connection.js", "device-binding.js"]) {
    vm.runInContext(localScript(file), scope);
  }
  const rows = (...devices) => { scope.ga.value = devices; scope.Pa.value = devices; notify(); };
  const scan = scope.pdConnection;
  return {scope, scan, rows, opened, routes, notify};
}

test("only the fully identified single device enters the original editor after the completed scan", () => {
  const s = screen();
  s.scan.begin(1);
  s.rows(keyboard);
  s.scan.begin(2);
  s.scan.finish(1, true); s.notify();
  assert.deepEqual(s.opened, []);
  s.scope.sa.initialized = false;
  s.scan.finish(2, true); s.notify();
  assert.deepEqual(s.opened, []);
  s.scope.sa.initialized = true; s.notify();
  assert.deepEqual(s.opened, [keyboard]);
  s.scope.window.PeripheralDeskHub.chooseDevice();
  s.scan.begin(3); s.scan.finish(3, true); s.notify();
  assert.deepEqual(s.routes, ["ConnectDevice"]);
  assert.deepEqual(s.opened, [keyboard]);
});

test("multiple devices, unresolved or sleeping devices retain the official cards", () => {
  for (const cards of [[keyboard, mouse], [{source:"receiver", identificationState:"pending"}],
    [{productKey:"G75_Pro", state:"deactivated"}], [], [{...keyboard, identificationState:"failed"}]]) {
    const s = screen();
    s.scan.begin(1); s.rows(...cards); s.scan.finish(1, true); s.notify();
    assert.deepEqual(s.opened, []);
  }
  const s = screen();
  s.scan.begin(1); s.rows(keyboard, mouse); s.scope.Pa.value = [keyboard];
  s.scan.finish(1, true); s.notify(); s.rows(keyboard);
  assert.deepEqual(s.opened, []);
});

test("the native switch works on a saved edition and suppresses an in-flight automatic entry", () => {
  const s = screen();
  s.scan.begin(1); s.rows(mouse); s.scope.window.PeripheralDeskHub.chooseDevice();
  s.scan.finish(1, true); s.notify();
  assert.deepEqual(s.opened, []);
  assert.deepEqual(s.routes, ["ConnectDevice"]);
});
