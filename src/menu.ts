export interface MenuItem {
  label: string;
  run(): void;
  danger?: boolean;
}

/** A small menu at the pointer (right-click); any click elsewhere or Esc closes it. */
export function openMenu(x: number, y: number, items: MenuItem[]) {
  document.querySelector(".menu")?.remove();
  const menu = document.createElement("div");
  menu.className = "menu";
  for (const it of items) {
    const b = document.createElement("button");
    b.textContent = it.label;
    if (it.danger) b.className = "danger";
    b.onclick = () => {
      close();
      it.run();
    };
    menu.append(b);
  }
  document.body.append(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, innerWidth - r.width - 6)}px`;
  menu.style.top = `${Math.min(y, innerHeight - r.height - 6)}px`;
  const away = (e: Event) => {
    if (!menu.contains(e.target as Node)) close();
  };
  const esc = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };
  function close() {
    menu.remove();
    document.removeEventListener("mousedown", away, true);
    document.removeEventListener("keydown", esc, true);
  }
  setTimeout(() => {
    document.addEventListener("mousedown", away, true);
    document.addEventListener("keydown", esc, true);
  });
}
