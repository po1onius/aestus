import { Loader2, Upload } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Modal } from "../../components/Modal";
import {
  buttonPrimary,
  fieldLabel,
  fieldStack,
  inputClass,
  requiredMark,
  spinnerClass,
  textareaClass,
} from "../../lib/ui";
import type { UpstreamApiKeyProvider } from "../../types";
import {
  emptyPluginArtifactFiles,
  hasPluginArtifact,
  PluginArtifactFileFields,
  type PluginArtifactFiles,
} from "./PluginArtifactFileFields";

export interface CreatePluginInput extends PluginArtifactFiles {
  name: string;
  description: string;
  provider: UpstreamApiKeyProvider;
}

interface PluginCreateDialogProps {
  saving: boolean;
  onCreate: (input: CreatePluginInput) => Promise<boolean>;
  onClose: () => void;
}

/** 添加弹窗负责采集一个完整的插件初始版本，成功发布后再由列表刷新展示结果。 */
export function PluginCreateDialog({ saving, onCreate, onClose }: PluginCreateDialogProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [provider, setProvider] = useState<UpstreamApiKeyProvider>("gpt");
  const [files, setFiles] = useState<PluginArtifactFiles>(emptyPluginArtifactFiles);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (saving || name.trim().length === 0 || !hasPluginArtifact(files)) {
      return;
    }
    if (await onCreate({ name, description, provider, ...files })) {
      onClose();
    }
  }

  return (
    <Modal
      titleId="pluginCreateTitle"
      title="添加插件"
      className="max-w-4xl"
      closeDisabled={saving}
      onClose={onClose}
    >
      <form className="grid gap-4" onSubmit={submit}>
        <div className="grid gap-4 sm:grid-cols-2">
          <label className={fieldStack}>
            <span className={fieldLabel}>
              插件名称<span className={requiredMark}>*</span>
            </span>
            <input
              className={inputClass}
              value={name}
              disabled={saving}
              maxLength={128}
              required
              autoFocus
              onChange={(event) => setName(event.target.value)}
              placeholder="例如 codex-wire-adapter"
            />
          </label>
          <label className={fieldStack}>
            <span className={fieldLabel}>
              Provider<span className={requiredMark}>*</span>
            </span>
            <select
              className={inputClass}
              value={provider}
              disabled={saving}
              onChange={(event) => setProvider(event.target.value as UpstreamApiKeyProvider)}
            >
              <option value="gpt">GPT · Responses</option>
              <option value="claude">Claude · Messages</option>
            </select>
          </label>
        </div>
        <label className={fieldStack}>
          <span className={fieldLabel}>描述</span>
          <textarea
            className={textareaClass}
            value={description}
            disabled={saving}
            maxLength={1024}
            onChange={(event) => setDescription(event.target.value)}
            placeholder="说明插件对请求、非流式响应和 SSE item 的转换规则"
          />
        </label>
        <PluginArtifactFileFields
          files={files}
          disabled={saving}
          layout="wide"
          onChange={setFiles}
        />
        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={saving || name.trim().length === 0 || !hasPluginArtifact(files)}
        >
          {saving ? <Loader2 className={spinnerClass} size={18} /> : <Upload size={18} />}
          添加并发布
        </button>
      </form>
    </Modal>
  );
}
