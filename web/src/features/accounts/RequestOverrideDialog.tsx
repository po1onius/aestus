import { Loader2, Save } from "lucide-react";
import type { FormEvent } from "react";
import { Modal } from "../../components/Modal";
import { OverrideKvEditor } from "../../components/OverrideKvEditor";
import { buttonPrimary, spinnerClass } from "../../lib/ui";
import type { OverrideEntry, RequestOverrideTarget } from "../../types";

interface RequestOverrideDialogProps {
  target: RequestOverrideTarget;
  headerRows: OverrideEntry[];
  bodyRows: OverrideEntry[];
  saving: boolean;
  onAdd: (section: "header" | "body") => void;
  onChange: (
    section: "header" | "body",
    id: string,
    field: "key" | "value",
    value: string,
  ) => void;
  onRemove: (section: "header" | "body", id: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onClose: () => void;
}

export function RequestOverrideDialog(props: RequestOverrideDialogProps) {
  const targetLabel =
    props.target.kind === "account"
      ? props.target.item.email || props.target.item.account_id || props.target.item.id
      : props.target.kind === "claudeAccount"
        ? props.target.item.email || props.target.item.account_uuid || props.target.item.id
        : props.target.item.masked_api_key;

  return (
    <Modal
      titleId="requestOverrideTitle"
      title="请求覆盖"
      description={targetLabel}
      className="max-w-6xl"
      closeDisabled={props.saving}
      onClose={props.onClose}
    >
      <div>
        <form className="grid gap-5" onSubmit={props.onSubmit}>
          <div className="grid items-start gap-4 lg:grid-cols-2">
            <OverrideKvEditor
              title="Header"
              rows={props.headerRows}
              disabled={props.saving}
              onAdd={() => props.onAdd("header")}
              onChange={(id, field, value) => props.onChange("header", id, field, value)}
              onRemove={(id) => props.onRemove("header", id)}
            />
            <OverrideKvEditor
              title="Body"
              rows={props.bodyRows}
              disabled={props.saving}
              onAdd={() => props.onAdd("body")}
              onChange={(id, field, value) => props.onChange("body", id, field, value)}
              onRemove={(id) => props.onRemove("body", id)}
            />
          </div>
          <div className="flex justify-end">
            <button type="submit" className={buttonPrimary} disabled={props.saving}>
              {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
              保存覆盖
            </button>
          </div>
        </form>
      </div>
    </Modal>
  );
}
