const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

test("editing a key preserves the board's other bindings and hidden slots", async () => {
    const navigator = {};
    const factory = vm.runInNewContext(
        fs.readFileSync(path.join(__dirname, "g75-keymap.js"), "utf8") + "\ncreateG75Editor",
        { navigator },
    );
    const layout = ["A", "Q"].map((key, index) => ({
        key, name: key, defaultVal: [0, 0, 0, 4 + index], fnVal: [0, 0, 0, 0],
    }));
    const fields = ["A", "Q", "noChange0"].map(key => ({ key, length: 4 }));
    const editor = factory(layout, fields, [], {space15: Array(126).fill(0), fix: [90, 165]});
    const board = {
        A: [0, 0, 0, 4], Q: [1, 9, 141, 208], noChange0: [50, 64, 77, 95],
        pdG75Tail: Array.from({ length: 120 }, (_, i) => 2 * i),
    };
    const writes = [];
    const handler = navigator.deviceHandler = {
        async sendCommand(mode, _command, frame, options) {
            if (mode === "get") return {...writes.at(-1).frame, pdG75Tail:board.pdG75Tail};
            writes.push(JSON.parse(JSON.stringify({frame,options})));
        },
    };
    const rows = editor.receive(handler, "Default", "defaultVal", board);
    assert.match(rows[1].name, /自定义/);
    rows[0].defaultVal = [0, 0, 0, 59];
    await editor.save(handler, "Default", rows[0]);
    assert.deepEqual(writes[0].frame.Q, board.Q);
    assert.deepEqual(writes[0].frame.noChange0, board.noChange0);
    assert.deepEqual(writes[0].frame.space15.slice(0, 120), board.pdG75Tail);
    rows[1].defaultVal = [0, 0, 0, 60];
    await editor.save(handler, "Default", rows[1]);
    assert.deepEqual(writes[1].frame.A, rows[0].defaultVal);
    assert.equal(writes[1].options.yy, 14);

    editor.beginRead();
    assert.throws(() => editor.receive(handler, "Fn", "fnVal", {...board, pdG75Tail: []}), /重新读取/);
    await assert.rejects(editor.save(handler, "Fn", rows[0]), /当前层/);
    const fnRows = editor.receive(handler, "Fn", "fnVal", board);
    await assert.rejects(editor.save(handler, "Default", rows[0]), /当前层/);
    handler.sendCommand = async () => { throw Error("connection lost"); };
    await assert.rejects(editor.save(handler, "Fn", fnRows[0]), /connection lost/);
    await assert.rejects(editor.save(handler, "Fn", fnRows[0]), /当前层/);
});

test("leaving while two G75 edits are queued reports each result after hardware readback", async () => {
    const messages = [];
    const scope = vm.createContext({
        navigator:{}, document:{addEventListener(){}},
        chrome:{webview:{postMessage:event=>messages.push(event)}}
    });
    scope.window = scope;
    vm.runInContext(fs.readFileSync(path.join(__dirname,"../hub/write-lifecycle.js"), "utf8"), scope);
    const make = vm.runInContext(fs.readFileSync(path.join(__dirname,"g75-keymap.js"),"utf8") + "\ncreateG75Editor",scope);
    const fields = ["A","B"].map(key=>({key,length:4}));
    const layout = fields.map(({key})=>({key,defaultVal:[0,0,0,0]}));
    const editor = make(layout, fields, [], {space15:Array(126).fill(0)});
    const board = {A:[0,0,0,0], B:[0,0,0,0], pdG75Tail:Array(120).fill(0x44)};
    let release;
    const firstWrite = new Promise(resolve=>{release=resolve;});
    const handler = scope.navigator.deviceHandler = {
        async sendCommand(mode, _, frame) {
            if (mode === "get") return board;
            await firstWrite;
            board.A = frame.A.slice();
            // Second edit is intentionally not accepted by hardware.
        }
    };
    const rows = editor.receive(handler,"Default","defaultVal",board);
    rows[0].defaultVal=[0,0,0,4]; rows[1].defaultVal=[0,0,0,5];
    const one=editor.save(handler,"Default",rows[0]);
    const two=editor.save(handler,"Default",rows[1]);
    const leave=scope.PeripheralDeskWrites.finishLeave();
    assert.deepEqual(messages.map(m=>m.event),["editable","queued","queued"]);
    await assert.rejects(editor.save(handler,"Default",rows[0]),/离开/);
    release();
    await one;
    await assert.rejects(two,/读回不一致/);
    await leave;
    assert.deepEqual(messages.slice(-3).map(m=>[m.event,m.id]),[["confirmed",1],["failed",2],["drained",undefined]]);
    assert.deepEqual(Array.from(board.A),[0,0,0,4]);
});
