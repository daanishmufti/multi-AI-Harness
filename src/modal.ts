// Minimal promise-based modal form builder. Returns the collected field values
// keyed by `key`, or null if the user cancels.

import { open } from "@tauri-apps/plugin-dialog";

export interface Field {
  key: string;
  label: string;
  type?: "text" | "textarea" | "select" | "checkbox" | "number" | "dir";
  value?: string | boolean | number;
  placeholder?: string;
  options?: { value: string; label: string }[];
}

export function modalForm(
  title: string,
  fields: Field[],
  submitLabel = "Create",
): Promise<Record<string, string> | null> {
  return new Promise((resolve) => {
    const root = document.getElementById("modal-root")!;
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";

    const modal = document.createElement("div");
    modal.className = "modal";
    const form = document.createElement("form");
    form.innerHTML = `<h2>${title}</h2>`;

    for (const f of fields) {
      if (f.type === "checkbox") {
        const wrap = document.createElement("label");
        wrap.className = "check";
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.name = f.key;
        cb.checked = !!f.value;
        wrap.append(cb, document.createTextNode(f.label));
        form.appendChild(wrap);
        continue;
      }
      if (f.type === "dir") {
        const label = document.createElement("label");
        label.textContent = f.label;
        form.appendChild(label);
        const row = document.createElement("div");
        row.className = "dir-row";
        const input = document.createElement("input");
        input.type = "text";
        input.name = f.key;
        if (f.value != null) input.value = String(f.value);
        if (f.placeholder) input.placeholder = f.placeholder;
        const browse = document.createElement("button");
        browse.type = "button";
        browse.textContent = "Browse…";
        browse.onclick = async () => {
          const picked = await open({ directory: true, multiple: false, title: f.label });
          if (typeof picked === "string") input.value = picked;
        };
        row.append(input, browse);
        form.appendChild(row);
        continue;
      }
      const label = document.createElement("label");
      label.textContent = f.label;
      form.appendChild(label);
      let input: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement;
      if (f.type === "textarea") {
        input = document.createElement("textarea");
      } else if (f.type === "select") {
        const sel = document.createElement("select");
        for (const o of f.options ?? []) {
          const opt = document.createElement("option");
          opt.value = o.value;
          opt.textContent = o.label;
          sel.appendChild(opt);
        }
        input = sel;
      } else {
        const i = document.createElement("input");
        i.type = f.type === "number" ? "number" : "text";
        input = i;
      }
      input.name = f.key;
      if (f.value != null) (input as HTMLInputElement).value = String(f.value);
      if (f.placeholder) (input as HTMLInputElement).placeholder = f.placeholder;
      form.appendChild(input);
    }

    const actions = document.createElement("div");
    actions.className = "modal-actions";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    const submit = document.createElement("button");
    submit.type = "submit";
    submit.textContent = submitLabel;
    actions.append(cancel, submit);
    form.appendChild(actions);
    modal.appendChild(form);
    backdrop.appendChild(modal);
    root.appendChild(backdrop);

    const close = (result: Record<string, string> | null) => {
      backdrop.remove();
      resolve(result);
    };
    cancel.onclick = () => close(null);
    backdrop.onclick = (e) => { if (e.target === backdrop) close(null); };
    form.onsubmit = (e) => {
      e.preventDefault();
      const out: Record<string, string> = {};
      for (const f of fields) {
        const node = form.elements.namedItem(f.key) as
          | HTMLInputElement
          | HTMLTextAreaElement
          | HTMLSelectElement
          | null;
        if (!node) continue;
        out[f.key] = f.type === "checkbox" ? String((node as HTMLInputElement).checked) : node.value;
      }
      close(out);
    };

    const first = form.querySelector("input, textarea, select") as HTMLElement | null;
    queueMicrotask(() => first?.focus());
  });
}
