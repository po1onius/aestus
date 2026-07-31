import { Loader2, PackageOpen } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { buttonPrimary, fieldHelp, spinnerClass } from "../../lib/ui";
import type { PluginReleaseSummary } from "../../types";
import {
  emptyPluginArtifactFiles,
  hasPluginArtifact,
  PluginArtifactFileFields,
  type PluginArtifactFiles,
} from "./PluginArtifactFileFields";

interface PluginReleaseDialogProps {
  plugin: PluginReleaseSummary;
  saving: boolean;
  onPublish: (suiteId: string, files: PluginArtifactFiles) => Promise<boolean>;
  onClose: () => void;
}

/** 新版本上传使用独立弹窗，避免文件输入和提示信息改变插件列表的行高与操作位置。 */
export function PluginReleaseDialog({
  plugin,
  saving,
  onPublish,
  onClose,
}: PluginReleaseDialogProps) {
  const [files, setFiles] = useState<PluginArtifactFiles>(emptyPluginArtifactFiles);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (saving || !hasPluginArtifact(files)) {
      return;
    }
    if (await onPublish(plugin.suite_id, files)) {
      onClose();
    }
  }

  return (
    <Modal
      titleId="pluginReleaseTitle"
      title="发布新版本"
      description={`${plugin.suite_name} 当前最新版本为 v${plugin.version}。`}
      className="max-w-4xl"
      closeDisabled={saving}
      onClose={onClose}
    >
      <form className="grid gap-4" onSubmit={submit}>
        <PluginArtifactFileFields
          files={files}
          disabled={saving}
          layout="wide"
          onChange={setFiles}
        />
        <button
          className={`${buttonPrimary} mt-1 w-full`}
          disabled={saving || !hasPluginArtifact(files)}
        >
          {saving ? (
            <Loader2 className={spinnerClass} size={18} />
          ) : (
            <PackageOpen size={18} />
          )}
          确认发布新版本
        </button>
      </form>
    </Modal>
  );
}
