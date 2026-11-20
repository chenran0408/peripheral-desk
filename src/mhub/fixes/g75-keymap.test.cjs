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
        async sendCommand(_mode, _command, frame, options) {
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
