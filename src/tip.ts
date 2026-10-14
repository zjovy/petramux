/** Where a tip of size `w`×`h` goes for a pointer at (x, y): below and right of it, kept inside the
 *  window (above the pointer when there's no room below). */
export function place(x: number, y: number, w: number, h: number, vw: number, vh: number): { left: number; top: number } {
  const left = Math.max(6, Math.min(x + 10, vw - w - 6));
  const top = y + 18 + h <= vh - 6 ? y + 18 : Math.max(6, y - h - 8);
  return { left, top };
}

const DELAY = 250; // the system's wait is about a second
const WARM = 600; // right after one closes, the next shows at once (as macOS does)

/** Quick tooltips in the theme's colors for everything with a `title`; the system's come late and
 *  look foreign. Each title moves to `data-tip` on hover, so the system's never shows. */
export function initTips() {
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.hidden = true;
  document.body.append(tip);
  let owner: HTMLElement | null = null;
  let timer: number | undefined;
  let closedAt = 0;
  let pointer = { x: 0, y: 0 };

  const show = () => {
    if (!owner?.isConnected) return;
    tip.textContent = owner.dataset.tip ?? "";
    tip.hidden = false;
    const r = tip.getBoundingClientRect();
    const at = place(pointer.x, pointer.y, r.width, r.height, innerWidth, innerHeight);
    tip.style.left = `${at.left}px`;
    tip.style.top = `${at.top}px`;
  };
  const hide = () => {
    clearTimeout(timer);
    if (!tip.hidden) closedAt = Date.now();
    tip.hidden = true;
    owner = null;
  };

  document.addEventListener("mouseover", (e) => {
    const el = (e.target as Element).closest?.<HTMLElement>("[title], [data-tip]") ?? null;
    if (el?.title) {
      el.dataset.tip = el.title;
      el.removeAttribute("title");
    }
    if (el === owner) return;
    const warm = !tip.hidden || Date.now() - closedAt < WARM;
    hide();
    if (!el?.dataset.tip) return;
    owner = el;
    pointer = { x: e.clientX, y: e.clientY };
    if (warm) show();
    else timer = window.setTimeout(show, DELAY);
  });
  document.addEventListener("mousemove", (e) => {
    if (tip.hidden) pointer = { x: e.clientX, y: e.clientY }; // it shows where the pointer rests
  });
  for (const type of ["mousedown", "keydown", "dragstart", "wheel"]) document.addEventListener(type, hide, true);
  document.addEventListener("mouseleave", hide);
  window.addEventListener("blur", hide);
}
