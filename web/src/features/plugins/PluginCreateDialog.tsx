import { Loader2, Upload } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { buttonPrimary, fieldLabel, fieldStack, inputClass, spinnerClass, textareaClass } from "../../lib/ui";
import type { PluginSlot, UpstreamApiKeyProvider } from "../../types";
import { pluginSlotLabels } from "./slots";

export interface CreatePluginInput {
  name: string;
  description: string;
  provider: UpstreamApiKeyProvider;
  slot: PluginSlot;
  file: File;
}

interface Props {
  isPlatformAdmin: boolean;
  saving: boolean;
  onCreate: (input: CreatePluginInput) => Promise<boolean>;
  onClose: () => void;
}

export function PluginCreateDialog({ saving, onCreate, onClose, isPlatformAdmin }: Props) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [provider, setProvider] = useState<UpstreamApiKeyProvider>("gpt");
  const [slot, setSlot] = useState<PluginSlot>("request");
  const [file, setFile] = useState<File | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (saving || !name.trim() || !file) return;
    if (await onCreate({ name, description, provider, slot, file })) onClose();
  }

  return (
    <Modal titleId="pluginCreateTitle" title={isPlatformAdmin ? "上传平台公共插件" : "上传本租户插件"} closeDisabled={saving} onClose={onClose}>
      <form className="grid gap-4" onSubmit={submit}>
        <label className={fieldStack}>
          <span className={fieldLabel}>插件名称</span>
          <input className={inputClass} value={name} disabled={saving} maxLength={128} required autoFocus onChange={(e) => setName(e.target.value)} />
        </label>
        <div className="grid gap-4 sm:grid-cols-2">
          <label className={fieldStack}>
            <span className={fieldLabel}>Provider</span>
            <select className={inputClass} value={provider} disabled={saving} onChange={(e) => setProvider(e.target.value as UpstreamApiKeyProvider)}>
              <option value="gpt">GPT · Responses</option>
              <option value="claude">Claude · Messages</option>
            </select>
          </label>
          <label className={fieldStack}>
            <span className={fieldLabel}>类型（插槽）</span>
            <select className={inputClass} value={slot} disabled={saving} onChange={(e) => setSlot(e.target.value as PluginSlot)}>
              {Object.entries(pluginSlotLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </select>
          </label>
        </div>
        <label className={fieldStack}>
          <span className={fieldLabel}>WASM 文件</span>
          <input className={inputClass} type="file" accept=".wasm,application/wasm" required disabled={saving} onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
          <span className="text-xs text-slate-500">文件最大 8 MiB。上传后可在多个套件中重复选择。</span>
        </label>
        <label className={fieldStack}>
          <span className={fieldLabel}>备注</span>
          <textarea className={textareaClass} value={description} disabled={saving} maxLength={1024} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <button className={`${buttonPrimary} w-full`} disabled={saving || !name.trim() || !file}>
          {saving ? <Loader2 className={spinnerClass} size={18} /> : <Upload size={18} />}上传插件
        </button>
      </form>
    </Modal>
  );
}
