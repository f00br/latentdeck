import { app } from "../../scripts/app.js";

const PICKERS = {
  LatentDeckToolkitLCLoadInspect: {
    widget: "lc_file",
    accept: ".lc",
    suffix: ".lc",
    subfolder: "latentdeck/cartridges",
    label: "Upload .lc from this computer",
  },
  LatentDeckToolkitRawH3Import: {
    widget: "safetensors_file",
    accept: ".safetensors",
    suffix: ".safetensors",
    subfolder: "latentdeck/raw",
    label: "Upload H3 .safetensors from this computer",
  },
};

function chooseOneFile(accept) {
  return new Promise((resolve) => {
    const picker = document.createElement("input");
    picker.type = "file";
    picker.accept = accept;
    picker.multiple = false;
    picker.onchange = () => resolve(picker.files?.[0] ?? null);
    picker.click();
  });
}

async function uploadComfyInput(file, subfolder) {
  const body = new FormData();
  body.append("image", file, file.name);
  body.append("type", "input");
  body.append("subfolder", subfolder);
  const response = await fetch("/upload/image", { method: "POST", body });
  if (!response.ok) {
    throw new Error(
      `Comfy input upload failed (${response.status} ${response.statusText})`,
    );
  }
  const result = await response.json();
  if (typeof result.name !== "string" || typeof result.subfolder !== "string") {
    throw new Error("Comfy input upload returned an invalid response");
  }
  return result.subfolder ? `${result.subfolder}/${result.name}` : result.name;
}

app.registerExtension({
  name: "LatentDeck.SafeCartridgeInputPicker",
  async nodeCreated(node) {
    const config = PICKERS[node.comfyClass];
    if (!config) return;

    const selection = node.widgets?.find(
      (widget) => widget.name === config.widget,
    );
    if (!selection) return;

    node.addWidget("button", config.label, null, async () => {
      try {
        const file = await chooseOneFile(config.accept);
        if (!file) return;
        if (!file.name.toLowerCase().endsWith(config.suffix)) {
          throw new Error(`Choose a ${config.suffix} file`);
        }
        const uploaded = await uploadComfyInput(file, config.subfolder);
        const values = selection.options?.values;
        if (Array.isArray(values) && !values.includes(uploaded))
          values.push(uploaded);
        selection.value = uploaded;
        selection.callback?.(uploaded);
        node.setDirtyCanvas(true, true);
      } catch (error) {
        window.alert(error instanceof Error ? error.message : String(error));
      }
    });
  },
});
