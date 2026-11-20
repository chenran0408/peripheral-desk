const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const source = fs.readFileSync(path.join(__dirname, "native-transport.js"), "utf8");
const local = "https://mhub.peripheraldesk.test";
const channel = "peripheral-desk-hid";
const collection = (usagePage, reportId) => ({
  usagePage, usage:1, type:1, children:[], inputReports:[{reportId, items:[]}],
  outputReports:[{reportId, items:[]}], featureReports:[{reportId, items:[]}]
});
const keyboard = {
  id:"interface-a", vendorId:0x3837, productId:1, productName:"G75 fixture",
  collections:[collection(0xff01, 19), collection(0xff02, 6)]
};
const other = {...keyboard, id:"interface-b"};
function load(origin, bus, top, browser = {browser:true}) {
  const context = vm.createContext({
    location:{origin}, navigator:{hid:browser}, EventTarget, Event, DOMException,
    chrome:bus && {webview:bus}
  });
  context.window = context;
  context.top = top || context;
  vm.runInContext(source, context);
  return context;
}
function fixture(browser) {
  const bus = new EventTarget();
  const sent = [];
  let available = [keyboard, other];
  const post = data => bus.dispatchEvent(Object.assign(new Event("message"), {data:{channel, ...data}}));
  bus.postMessage = message => {
    assert.equal(message.channel, channel);
    sent.push(message);
    let value = null;
    if (message.op === "list") value = available;
    if (message.op === "receiveFeature") value = message.reportId ? [6,0x83,1,2] : [0x83,4];
    queueMicrotask(() => post({id:message.id,ok:true,value}));
  };
  const top = load(local,bus,undefined,browser);
  return {bus, top, hid:top.navigator.hid, sent, post, setList(list) { available = list; }};
}

test("the original home receives both scoped devices; filters match collections and retain identities", async () => {
  const f = fixture();
  const [all, second] = await Promise.all([f.hid.getDevices(), f.hid.getDevices()]);
  assert.equal(all.length,2);
  assert.notEqual(all[0], all[1]);
  assert.notEqual(all,second);
  assert.equal(all[0],second[0]);
  assert.equal(f.sent.length,1);
  all[0].id = "keyboard-G75"; // The original editor sets its own writable model id.
  const matched = await f.hid.requestDevice({filters:[{vendorId:0x3837,usagePage:0xff02,usage:1}]});
  assert.equal(matched[0],all[0]);
  assert.equal((await f.hid.requestDevice({filters:[{usagePage:0xff00}]})).length,0);
  assert.equal((await f.hid.requestDevice({filters:[],exclusionFilters:[{productId:1,vendorId:0x3837}]})).length,0);
  assert.equal(all[0].collections[1].featureReports[0].reportId,6);
  await all[0].open();
  assert.equal(f.sent.at(-1).device,"interface-a");
  assert.deepEqual(f.sent.filter(m=>m.op==="open").map(m=>m.device),["interface-a"]);
});

test("wired feature reads retain Windows bytes; wireless input and sliced output use WebHID offsets", async () => {
  const f = fixture();
  const [device] = await f.hid.getDevices();
  await assert.rejects(device.sendReport(19,new Uint8Array([1])),{name:"InvalidStateError"});
  await Promise.all([device.open(),device.open()]);
  assert.equal(f.sent.filter(m=>m.op==="open").length,1);
  const backing = Uint8Array.of(99,5,6,7,88);
  await device.sendFeatureReport("0x06", new DataView(backing.buffer,1,3));
  assert.deepEqual(Array.from(f.sent.at(-1).data),[5,6,7]);
  assert.equal(f.sent.at(-1).reportId,6);
  const feature = await device.receiveFeatureReport(6);
  assert.deepEqual([...new Uint8Array(feature.buffer)],[6,0x83,1,2]);
  assert.deepEqual([...new Uint8Array((await device.receiveFeatureReport(0)).buffer)],[0x83,4]);
  await device.sendReport(19,backing.subarray(2,4));
  assert.equal(f.sent.at(-1).op,"sendOutput");
  assert.deepEqual(Array.from(f.sent.at(-1).data),[6,7]);
  await assert.rejects(device.sendReport(256,backing),{name:"TypeError"});
  const events=[];
  const listener=e=>{events.push([e.reportId,e.device===device,e.target===device,...new Uint8Array(e.data.buffer)]);};
  device.addEventListener("inputreport",listener);
  let property=0;
  device.oninputreport=()=>{property++;};
  f.post({op:"input",device:keyboard.id,reportId:19,data:[0x41,0x1]});
  assert.deepEqual(events,[[19,true,true,0x41,1]]);
  assert.equal(property,1);
  device.removeEventListener("inputreport",listener);
  device.oninputreport=null;
  await device.close();
  assert.equal(device.opened,false);
  f.post({op:"input",device:keyboard.id,reportId:19,data:[3]});
  assert.equal(events.length,1);
});

test("hotplug binds to the right physical unit and closed handles cannot reappear after unplug", async () => {
  const f=fixture();
  const [a,b]=await f.hid.getDevices();
  await Promise.all([a.open(), b.open()]);
  let unplug, plug;
  f.hid.ondisconnect=e=>{unplug=e.device;};
  f.hid.addEventListener("connect",e=>{plug=e.device;});
  f.post({op:"disconnect",device:keyboard.id});
  f.setList([other]);
  assert.equal(unplug,a);
  assert.equal(a.opened,false);
  assert.equal(b.opened,true);
  assert.equal((await f.hid.getDevices())[0],b);
  await assert.rejects(a.open(),{name:"InvalidStateError"});
  f.post({op:"connect",descriptor:keyboard});
  f.setList([keyboard,other]);
  assert.notEqual(plug,a);
  assert.equal((await f.hid.getDevices())[0],plug);
  assert.equal(plug.opened,false);
});

test("a trusted child shares the active native session; unrelated origins retain browser HID", async () => {
  const f=fixture();
  const child=load(local,null,f.top);
  assert.equal(child.navigator.hid, f.hid);
  assert.equal(load("https://www.example.com",f.bus).navigator.hid.browser,true);
  assert.equal(load(local,null).navigator.hid.browser,true);
  assert.equal(load(local,null,load("https://other.example",f.bus)).navigator.hid.browser,true);
  assert.equal((await child.navigator.hid.getDevices())[0],(await f.hid.getDevices())[0]);
});

test("native errors reject the matching request and do not leave a phantom opened device", async () => {
  const f=fixture();
  const [dev] = await f.hid.getDevices();
  f.bus.postMessage=m=>queueMicrotask(()=>f.post({id:m.id,ok:false,error:"设备已拔出"}));
  await assert.rejects(dev.open(),{name:"NetworkError",message:"设备已拔出"});
  assert.equal(dev.opened,false);
});

test("previous browser grants survive, overlapping physical twins appear once each, and gallery pairing retains activation", async () => {
  const browser = new EventTarget();
  const originalA = {...keyboard, collections:[collection(1,1), ...keyboard.collections]};
  const originalB = {...keyboard};
  const thirdTwin = {...keyboard};
  const magnetic = {...keyboard,productName:"Magnetic fixture", productId:7};
  const extraInterface = {...keyboard,collections:[collection(0xff09, 73)]};
  const picked = {...keyboard,productName:"Other supported fixture",productId:9};
  let grants = [originalA,originalB,thirdTwin,magnetic,extraInterface];
  browser.getDevices = async () => grants;
  let prompts = 0;
  let response = [picked];
  browser.requestDevice = async () => {
    prompts++;
    grants = [...new Set([...grants, ...response])];
    return response;
  };
  const f = fixture(browser);
  const list = await f.hid.getDevices();
  assert.equal(list.length,5);
  assert.deepEqual([...list.slice(2)], [thirdTwin,magnetic,extraInterface]);
  assert.equal(prompts,0);
  grants.reverse();
  const exact = {filters:[{vendorId:keyboard.vendorId,productId:keyboard.productId,usagePage:0xff01}]};
  const known = await f.hid.requestDevice(exact);
  assert.deepEqual([...known],[list[0],list[1],thirdTwin]);
  assert.equal(prompts,0);
  const broad = {filters:[{vendorId:keyboard.vendorId}]};
  const choosing = f.hid.requestDevice(broad);
  assert.equal(prompts,1,"invoke the browser picker in the original click");
  assert.deepEqual([...(await choosing)],[picked]);
  response=[];
  assert.equal((await f.hid.requestDevice(broad)).length,0,"cancellation stays empty");
  response=[originalB];
  assert.deepEqual([...(await f.hid.requestDevice(broad))],[list[1]]);
});

test("hotplug forwards unsupported browser devices and shares a single event for overlapping devices", async () => {
  const browser = new EventTarget();
  const original = {...keyboard};
  const magnetic = {...keyboard, productId:21};
  browser.getDevices = async () => [original,magnetic];
  const f=fixture(browser);
  const [native]=await f.hid.getDevices();
  const events=[];
  f.hid.onconnect=e=>events.push(["connect",e.device]);
  f.hid.ondisconnect=e=>events.push(["disconnect",e.device]);
  const browserEvent=(type,device)=>browser.dispatchEvent(Object.assign(new Event(type),{device}));
  browserEvent("connect",magnetic);
  browserEvent("connect",original);
  f.post({op:"connect",descriptor:keyboard});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(events.length,2);
  assert.ok(events.some(([op,d])=>op==="connect"&&d===native));
  assert.ok(events.some(([op,d])=>op==="connect"&&d===magnetic));
  browserEvent("disconnect",magnetic);
  browserEvent("disconnect",original);
  f.post({op:"disconnect",device:keyboard.id});
  assert.deepEqual(events.slice(2),[["disconnect",magnetic],["disconnect",native]]);
});
