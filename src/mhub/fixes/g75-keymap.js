// The G75 Pro writes a whole layer even when only one key is edited.
// Keep the receiver's map, including unknown bindings and reserved slots.
function createG75Editor(layout, protocolFields, choices, writeConstants) {
    let session;
    let pending = Promise.resolve();

    function receive(handler, layer, field, response) {
        const raw = {};
        for (const { key, length } of protocolFields) {
            const bytes = response[key];
            if (!bytes || bytes.length !== length) {
                throw new Error("G75 Pro 的板载按键表未读完整，请唤醒键盘后重新进入改键页。");
            }
            raw[key] = Array.from(bytes);
        }
        // G75 returns 126 four-byte slots. Its public layout describes 96.
        // Retain the other 30 from hardware and keep the vendor frame trailer.
        if (response.pdG75Tail?.length !== 120) {
            throw new Error("G75 Pro 的板载按键表缺少尾部数据，请重新读取后再编辑。");
        }
        raw.space15 = [...response.pdG75Tail, ...writeConstants.space15.slice(120)];
        const rows = layout.map(item => {
            const bytes = raw[item.key];
            const row = { ...item, [field]: bytes.slice() };
            if (bytes.join() === item[field].join()) return row;
            const choice = choices.find(entry => entry.val?.join() === bytes.join());
            row.name = choice?.name || (bytes[0] === 3
                ? "宏 #" + (bytes[3] + 1)
                : "自定义 " + bytes.map(byte => byte.toString(16).padStart(2, "0")).join(" "));
            return row;
        });
        session = { handler, layer, field, raw };
        globalThis.window?.PeripheralDeskWrites?.editable();
        return rows;
    }

    function save(handler, layer, row, preparation) {
        const active = session;
        if (!active || active.handler !== handler || active.layer !== layer || !row) {
            return Promise.reject(new Error("请先读取 G75 Pro 当前层的板载按键。"));
        }
        const key = row.key;
        const bytes = row[active.field].slice();
        const send = async () => {
            if (session !== active || handler !== navigator.deviceHandler) {
                throw new Error("设备或按键层已切换，请在当前页面重新修改。");
            }
            const layerIndex = { Default: 0, Fn: 1, Fn2: 2 }[layer];
            const frame = {
                ...writeConstants, ...active.raw, [key]: bytes,
                space0: [layerIndex, 0, 1, 0, 248, 1]
            };
            try {
                await handler.sendCommand("set", "setKeySetting", frame, { yy: 14 + 16 * layerIndex });
                const current = await handler.sendCommand("get", "getKeySetting", {});
                for (const {key: slot} of protocolFields) {
                    if (!current[slot] || current[slot].join() !== frame[slot].join())
                        throw new Error("G75 Pro 板载键位读回不一致，请重新读取本层。");
                }
                if (current.pdG75Tail?.join() !== frame.space15.slice(0,120).join())
                    throw new Error("G75 Pro 保留槽读回不一致，请重新读取本层。");
            } catch (error) {
                if (session === active) session = undefined;
                throw error;
            }
            active.raw[key] = bytes;
        };
        const prior = pending;
        const ready = Promise.all([prior, preparation]);
        const enqueue = () => ready.then(send);
        const host = globalThis.window?.PeripheralDeskWrites;
        const write = host ? host.track(enqueue) : enqueue();
        pending = write.catch(() => {});
        return write;
    }
    return {
        receive, save,
        withTail(makeParser) {
            return async () => (await makeParser()).array("pdG75Tail", {type: "uint8", length: 120});
        },
        beginRead() { session = undefined; },
        waitForWrites() { return pending; }
    };
}
