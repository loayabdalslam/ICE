const KEY = "ice-freeze-tasks";
const list = document.getElementById("list");
const stats = document.getElementById("stats");
const title = document.getElementById("title");

const load = () => JSON.parse(localStorage.getItem(KEY) || "[]");
const save = (items) => localStorage.setItem(KEY, JSON.stringify(items));

function render() {
  const items = load();
  list.innerHTML = "";
  items.forEach((item, i) => {
    const li = document.createElement("li");
    if (item.done) li.classList.add("done");
    li.innerHTML = `<input type="checkbox" ${item.done ? "checked" : ""} /><span></span>`;
    li.querySelector("span").textContent = item.title;
    li.querySelector("input").onchange = () => {
      items[i].done = !items[i].done;
      save(items);
      render();
    };
    list.appendChild(li);
  });
  const open = items.filter((x) => !x.done).length;
  stats.textContent = `${open} open · ${items.length} frozen`;
}

document.getElementById("add").onsubmit = (e) => {
  e.preventDefault();
  const text = title.value.trim();
  if (!text) return;
  save([{ title: text, done: false }, ...load()]);
  title.value = "";
  render();
};

document.getElementById("clear").onclick = () => {
  save(load().filter((x) => !x.done));
  render();
};

render();
