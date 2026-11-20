const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const script = name => fs.readFileSync(path.join(__dirname, "../hub", name), "utf8");
const keyboard = {productKey:"by-55", deviceName:"G75_Pro", productName:"Wireless Receiver", type:"keyboard", identificationState:"resolved"};
const mouse = {productKey:"ms-0c", deviceName:"A5 Ultra", productName:"A5 ultra", type:"mouse"};
const later = {productKey:"ms-2c", deviceName:"A5 V2 Ultra", productName:"MCHOSE A5 V2 Ultra", type:"mouse"};

function screen(preferred="", data=new Map()) {
  const opened = [], routes = [];
  let notify;
  const router = {push:to => routes.push(to.name)};
  const scope = {
    window:{__peripheralDeskPreferredModel:preferred}, document:{querySelector:() => ({__vue_app__:{config:{globalProperties:{$router:router}}}})},
    $a:router, localStorage:{getItem:key=>data.get(key)??null, setItem:(key,value)=>data.set(key,value)},
    ref:value => ({value}), watch:(_, callback) => { notify = callback; },
    Ua:{value:true}, sa:{initialized:true},
    ga:{value:[]}, Ma:{value:[]}, Ca:{value:[]}, Pa:{value:[]},
    ui:card=>card.source==="audio", Ai:card=>card.state||"normal", os:card=>opened.push(card),
    getHomeDeviceRuntimeKey:card=>card.logicalDeviceId||card.productKey,
    getConnectionKind:card=>card.type
  };
  vm.createContext(scope);
  for (const file of ["switch-device.js", "device-connection.js", "device-binding.js"]) vm.runInContext(script(file), scope);
  const rows = (...devices) => { scope.ga.value=devices; scope.Pa.value=devices; notify(); };
  const scan = scope.pdConnection;
  const done = (...devices) => { scan.begin(1); rows(...devices); scan.finish(1,true); notify(); };
  return {scope, scan, rows, done, opened, routes, notify};
}

test("battery entry picks its exact model from two matching-family devices after the finished scan", () => {
  const s = screen("mchose-a5-ultra");
  s.scan.begin(1); s.rows(later);
  s.scan.begin(2); s.scan.finish(1,true); s.notify();
  s.rows(keyboard,later,mouse);
  s.scope.sa.initialized=false; s.scan.finish(2,true); s.notify();
  assert.deepEqual(s.opened,[]);
  s.scope.sa.initialized=true; s.notify();
  assert.deepEqual(s.opened,[mouse]);
  s.scope.window.PeripheralDeskHub.chooseDevice();
  s.done(keyboard,mouse);
  assert.deepEqual(s.routes,["ConnectDevice"]);
  assert.deepEqual(s.opened,[mouse]);
});

test("G75 identity ignores transport spelling, and unidentified/ambiguous receivers retain vendor selection", () => {
  const s=screen("mchose-g75-pro");s.done(mouse,keyboard);assert.deepEqual(s.opened,[keyboard]);
  const waiting=screen("mchose-a5-ultra");waiting.done(later,keyboard);assert.deepEqual(waiting.opened,[]);
  for (const devices of [[{source:"receiver",productKey:"G75_Pro",identificationState:"pending"}],
     [{...keyboard,state:"deactivated"}],[],[{...keyboard,identificationState:"failed"}],[keyboard,{...keyboard}]]) {
    const one=screen();one.done(...devices);assert.deepEqual(one.opened,[]);
  }
});

test("generic entry is stable across order and remembers a successfully opened manual model", () => {
  const data=new Map();
  const s=screen("",data);s.done(keyboard,mouse);assert.deepEqual(s.opened,[mouse]);
  const other=screen();other.done(mouse,keyboard);assert.deepEqual(other.opened,[mouse]);
  data.set("lastKBProductKey",keyboard.productKey);data.set("trackDeviceName","G75 Pro");
  s.scope.window.PeripheralDeskHub.chooseDevice();
  s.scan.arrived({name:"Keyboard",matched:[]},{name:"ConnectDevice"},{type:8});
  assert.equal(data.has("peripheraldesk.mhub.lastModel"),false);
  s.scan.arrived({name:"Keyboard",matched:[]},{name:"ConnectDevice"});
  const ret=screen("",data);ret.done(mouse,keyboard);assert.deepEqual(ret.opened,[keyboard]);
  const renamed={...keyboard,deviceName:"迈从无线键盘",productName:"2.4G Receiver"};
  const exact=screen("",data);exact.done(mouse,renamed);assert.deepEqual(exact.opened,[renamed]);
  const swapped={...keyboard,productKey:"by-wire"};const cable=screen("",data);cable.done(mouse,swapped);assert.deepEqual(cable.opened,[swapped]);
  const missing=screen("",data);missing.done(mouse);assert.deepEqual(missing.opened,[mouse]);
  const explicit=screen("mchose-a5-ultra",data);explicit.done(mouse,keyboard);assert.deepEqual(explicit.opened,[mouse]);
});

test("manual switch stays on gallery, next native selection can change model in the same view", async () => {
  const s=screen();
  s.scan.begin(1);s.rows(mouse,keyboard);s.scope.window.PeripheralDeskHub.chooseDevice();
  s.scan.finish(1,true);s.notify();assert.deepEqual(s.opened,[]);
  await s.scope.window.PeripheralDeskHub.openModel("mchose-g75-pro");s.notify();assert.deepEqual(s.opened,[keyboard]);
  s.scope.Ua.value=false;
  await s.scope.window.PeripheralDeskHub.openModel("mchose-a5-ultra");s.notify();
  s.scope.Ua.value=true;s.done(keyboard,mouse);assert.deepEqual(s.opened,[keyboard,mouse]);
  assert.deepEqual(s.routes,["ConnectDevice","ConnectDevice"]);
});
