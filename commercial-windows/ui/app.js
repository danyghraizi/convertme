const invoke = window.__TAURI__.core.invoke;
const state = { files: [], busy: false };
const $ = (id) => document.getElementById(id);

function renderFiles() {
  const list = $("file-list");
  list.replaceChildren();
  $("empty-state").hidden = state.files.length > 0;
  $("convert").disabled = state.files.length === 0 || state.busy;
  for (const [index, file] of state.files.entries()) {
    const row = document.createElement("li");
    row.className = "file-row";
    const icon = document.createElement("span");
    icon.className = "file-icon";
    icon.textContent = "▧";
    icon.setAttribute("aria-hidden", "true");
    const info = document.createElement("span");
    info.className = "file-info";
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = file.name;
    const path = document.createElement("span");
    path.className = "file-path";
    path.textContent = file.path;
    info.append(name, path);
    const remove = document.createElement("button");
    remove.className = "remove-file";
    remove.type = "button";
    remove.textContent = "×";
    remove.title = `Remove ${file.name}`;
    remove.addEventListener("click", () => {
      state.files.splice(index, 1);
      renderFiles();
    });
    row.append(icon, info, remove);
    list.append(row);
  }
}

$("add-files").addEventListener("click", async () => {
  try {
    const files = await invoke("pick_images");
    const known = new Set(state.files.map((file) => file.path.toLowerCase()));
    for (const file of files) {
      if (!known.has(file.path.toLowerCase())) state.files.push(file);
    }
    renderFiles();
    $("status").textContent = `${state.files.length} image${state.files.length === 1 ? "" : "s"} selected.`;
  } catch (error) {
    $("status").textContent = `Could not open the file picker: ${error}`;
  }
});

$("format").addEventListener("change", () => {
  $("quality-field").hidden = $("format").value !== "jpeg";
});
$("quality").addEventListener("input", () => {
  $("quality-value").value = $("quality").value;
});

$("convert").addEventListener("click", async () => {
  if (state.files.length === 0 || state.busy) return;
  state.busy = true;
  renderFiles();
  $("results").replaceChildren();
  $("status").textContent = `Converting ${state.files.length} image${state.files.length === 1 ? "" : "s"}…`;
  try {
    const result = await invoke("convert_images", {
      paths: state.files.map((file) => file.path),
      format: $("format").value,
      maxWidth: optionalNumber("width"),
      maxHeight: optionalNumber("height"),
      rotation: Number($("rotation").value),
      jpegQuality: Number($("quality").value),
    });
    for (const file of result.files) {
      const row = document.createElement("div");
      row.className = `result-row${file.error ? " failed" : ""}`;
      row.textContent = file.error ? `${file.source}: ${file.error}` : `Saved ${file.output}`;
      $("results").append(row);
    }
    $("status").textContent = `Finished: ${result.succeeded} converted, ${result.failed} failed.`;
  } catch (error) {
    $("status").textContent = `Conversion failed: ${error}`;
  } finally {
    state.busy = false;
    renderFiles();
  }
});

function optionalNumber(id) {
  const text = $(id).value.trim();
  if (text === "") return null;
  const value = Number(text);
  return Number.isInteger(value) && value > 0 ? value : null;
}

renderFiles();
