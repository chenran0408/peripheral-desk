// Host navigation hides this document while finishLeave waits for accepted edits.
(() => {
  const host = window.chrome?.webview;
  if (!host || (window.top && window !== window.top)) return;
  let leaving = false, next = 0;
  const jobs = new Set();
  const tell = (event, data={}) => host.postMessage({channel:"peripheral-desk-editor", event, ...data});
  window.PeripheralDeskWrites = {
    track(make) {
      if (leaving) return Promise.reject(Error("正在离开驱动页，请重新进入后修改。"));
      const id = ++next;
      tell("queued", {id});
      const work = Promise.resolve().then(make).then(value => {
        tell("confirmed", {id});
        return value;
      }, error => {
        tell("failed", {id});
        throw error;
      });
      jobs.add(work);
      work.then(() => jobs.delete(work), () => jobs.delete(work));
      return work;
    },
    async whenReady() { await Promise.allSettled([...jobs]); },
    async finishLeave() {
      leaving = true;
      await Promise.allSettled([...jobs]);
      tell("drained");
    },
    editable() { tell("editable"); }
  };
  if (window.__peripheralDeskMeasure) {
    const painted = () => {
      if (document.querySelector("#app")?.innerText.trim()) requestAnimationFrame(() => tell("frame"));
      else requestAnimationFrame(painted);
    };
    document.addEventListener("DOMContentLoaded", painted, {once:true});
  }
})();
