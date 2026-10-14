// Runs in an isolated world of each page of the Mac browser. On pages of this computer it keeps
// `<select>` and date, time and color inputs from opening their native pickers, which pictures of
// the page do not show, and reports them through the `hakoReport` binding instead; the client's
// choice comes back through `hakoPick`.
(() => {
  const host = location.hostname.replace(/\.$/, "").toLowerCase();
  const local =
    host === "localhost" ||
    host.endsWith(".localhost") ||
    /^127(\.\d{1,3}){3}$/.test(host) ||
    host === "[::1]" ||
    host === "0.0.0.0";
  // Once per page, should it be run into the same page twice.
  if (!local || typeof hakoReport !== "function" || globalThis.hakoPick) return;

  const INPUT_KINDS = ["date", "time", "datetime-local", "month", "week", "color"];
  const ids = new WeakMap();
  const elements = new Map();
  let next = 0;
  // A tap may come as a touch and then as the mouse events made from it.
  let lastReport = { el: null, at: 0 };

  const kindOf = (el) => {
    if (el instanceof HTMLSelectElement) return "select";
    if (el instanceof HTMLInputElement && INPUT_KINDS.includes(el.type)) return el.type;
    return null;
  };
  const idOf = (el) => {
    let id = ids.get(el);
    if (!id) {
      id = "p" + ++next;
      ids.set(el, id);
      elements.set(id, new WeakRef(el));
    }
    return id;
  };
  const pickerOf = (target) => {
    const el = target instanceof Element ? target.closest("select, input") : null;
    if (!el || el.disabled || !kindOf(el)) return null;
    if (el instanceof HTMLInputElement && el.readOnly) return null;
    // A list box shows its options on the page itself.
    if (el instanceof HTMLSelectElement && (el.multiple || el.size > 1)) return null;
    return el;
  };
  const report = (el) => {
    const now = performance.now();
    if (lastReport.el === el && now - lastReport.at < 500) return;
    lastReport = { el, at: now };
    const kind = kindOf(el);
    const message = { kind, id: idOf(el) };
    if (kind === "select") {
      message.multiple = el.multiple;
      message.options = [...el.options].map((option) => {
        const group = option.parentElement instanceof HTMLOptGroupElement ? option.parentElement : null;
        return {
          label: option.label || option.text,
          value: option.value,
          selected: option.selected,
          disabled: option.disabled || (group !== null && group.disabled),
          group: group ? group.label : null,
        };
      });
    } else {
      message.value = el.value;
      message.min = el.min || null;
      message.max = el.max || null;
      message.step = el.step || null;
    }
    hakoReport(JSON.stringify(message));
  };

  addEventListener(
    "mousedown",
    (event) => {
      const el = pickerOf(event.target);
      if (!el || event.button !== 0) return;
      event.preventDefault();
      el.focus();
      report(el);
    },
    true,
  );
  // With touch emulated, a tap can reach a picker without a mousedown: it is taken as the finger
  // lifts, unless the finger moved, as it does in a scroll that started on the field.
  let touchStart = null;
  addEventListener(
    "touchstart",
    (event) => {
      const touch = event.touches[0];
      touchStart =
        pickerOf(event.target) && touch && event.touches.length === 1
          ? { x: touch.clientX, y: touch.clientY }
          : null;
    },
    true,
  );
  addEventListener(
    "touchend",
    (event) => {
      const el = pickerOf(event.target);
      const touch = event.changedTouches[0];
      const start = touchStart;
      touchStart = null;
      if (!el || !start || !touch) return;
      if (Math.hypot(touch.clientX - start.x, touch.clientY - start.y) > 10) return;
      event.preventDefault();
      el.focus();
      report(el);
    },
    { capture: true, passive: false },
  );
  // The pickers of inputs open on click as well.
  addEventListener(
    "click",
    (event) => {
      const el = pickerOf(event.target);
      if (el && !(el instanceof HTMLSelectElement)) event.preventDefault();
    },
    true,
  );
  addEventListener(
    "keydown",
    (event) => {
      const el = pickerOf(event.target);
      if (!el) return;
      const opens =
        el instanceof HTMLSelectElement
          ? [" ", "Enter", "ArrowUp", "ArrowDown"].includes(event.key)
          : event.key === " " || (event.key === "ArrowDown" && event.altKey);
      if (!opens) return;
      event.preventDefault();
      report(el);
    },
    true,
  );

  globalThis.hakoPick = (id, values) => {
    const el = elements.get(id)?.deref();
    if (!el || !el.isConnected) return false;
    if (el instanceof HTMLSelectElement) {
      for (const option of el.options) option.selected = values.includes(option.value);
    } else {
      el.value = values.length > 0 ? values[0] : "";
    }
    el.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  };
})();
