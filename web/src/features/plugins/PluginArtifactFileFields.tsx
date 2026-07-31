import {
  fieldLabel,
  fieldStack,
  inputClass,
} from "../../lib/ui";

export interface PluginArtifactFiles {
  requestFile: File | null;
  bufferedResponseFile: File | null;
  streamResponseFile: File | null;
}

export function emptyPluginArtifactFiles(): PluginArtifactFiles {
  return {
    requestFile: null,
    bufferedResponseFile: null,
    streamResponseFile: null,
  };
}

export function hasPluginArtifact(files: PluginArtifactFiles) {
  return Boolean(files.requestFile || files.bufferedResponseFile || files.streamResponseFile);
}

interface PluginArtifactFileFieldsProps {
  files: PluginArtifactFiles;
  disabled: boolean;
  layout?: "stacked" | "wide";
  onChange: (files: PluginArtifactFiles) => void;
}

/** 创建插件和发布新版本共用同一组插槽输入，确保空插槽语义与文件限制提示一致。 */
export function PluginArtifactFileFields({
  files,
  disabled,
  layout = "stacked",
  onChange,
}: PluginArtifactFileFieldsProps) {
  const fields: Array<{ key: keyof PluginArtifactFiles; label: string; hint: string }> = [
    { key: "requestFile", label: "请求插件", hint: "原始下游请求 → 上游请求" },
    {
      key: "bufferedResponseFile",
      label: "非流式响应插件",
      hint: "完整上游响应 → 下游响应",
    },
    { key: "streamResponseFile", label: "流式响应插件", hint: "逐个完整原始 SSE item" },
  ];

  return (
    <div className={layout === "wide" ? "grid gap-4 md:grid-cols-3" : "grid gap-3"}>
      {fields.map((field) => (
        <label className={fieldStack} key={field.key}>
          <span className={fieldLabel}>{field.label}</span>
          <input
            className={inputClass}
            type="file"
            accept=".wasm,application/wasm"
            disabled={disabled}
            onChange={(event) =>
              onChange({ ...files, [field.key]: event.target.files?.[0] ?? null })
            }
          />
          <span className="text-xs text-slate-500 dark:text-slate-400">{field.hint}</span>
        </label>
      ))}
    </div>
  );
}
