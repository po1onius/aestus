import { Loader2, Save } from "lucide-react";
import { useState } from "react";
import { createOverrideEntry, overrideEntriesFromObject } from "./utils";
import { Modal } from "../../components/Modal";
import { OverrideKvEditor } from "../../components/OverrideKvEditor";
import { buttonPrimary, spinnerClass } from "../../lib/ui";
import type { OverrideEntry, RequestOverrideTarget } from "../../types";

interface RequestOverrideDialogProps {
  target: RequestOverrideTarget;
  saving: boolean;
  readOnly: boolean;
  onSubmit: (input: { headerRows: OverrideEntry[]; bodyRows: OverrideEntry[] }) => void;
  onClose: () => void;
}

export function RequestOverrideDialog(props: RequestOverrideDialogProps) {
  const [requestOverrideHeaderRows, setRequestOverrideHeaderRows] = useState(() => overrideEntriesFromObject(props.target.item.override!.header));
  const [requestOverrideBodyRows, setRequestOverrideBodyRows] = useState(() => overrideEntriesFromObject(props.target.item.override!.body));
  function addRequestOverrideRow(section: "header" | "body") {
    const append = (rows: OverrideEntry[]) => [...rows, createOverrideEntry("", "")];
    if (section === "header") {
      setRequestOverrideHeaderRows(append);
    } else {
      setRequestOverrideBodyRows(append);
    }
  }
  function updateRequestOverrideRow(
    section: "header" | "body",
    id: string,
    field: "key" | "value",
    value: string,
  ) {
    const update = (rows: OverrideEntry[]) =>
      rows.map((row) => (row.id === id ? { ...row, [field]: value } : row));
    if (section === "header") {
      setRequestOverrideHeaderRows(update);
    } else {
      setRequestOverrideBodyRows(update);
    }
  }
  function removeRequestOverrideRow(section: "header" | "body", id: string) {
    const remove = (rows: OverrideEntry[]) => rows.filter((row) => row.id !== id);
    if (section === "header") {
      setRequestOverrideHeaderRows(remove);
    } else {
      setRequestOverrideBodyRows(remove);
    }
  }

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
        <form className="grid gap-5" onSubmit={event => { event.preventDefault(); props.onSubmit({ headerRows: requestOverrideHeaderRows, bodyRows: requestOverrideBodyRows }); }}>
          <div className="grid items-start gap-4 lg:grid-cols-2">
            <OverrideKvEditor
              title="Header"
              rows={requestOverrideHeaderRows}
              disabled={props.saving || props.readOnly}
              onAdd={() => addRequestOverrideRow("header")}
              onChange={(id, field, value) => updateRequestOverrideRow("header", id, field, value)}
              onRemove={(id) => removeRequestOverrideRow("header", id)}
            />
            <OverrideKvEditor
              title="Body"
              rows={requestOverrideBodyRows}
              disabled={props.saving || props.readOnly}
              onAdd={() => addRequestOverrideRow("body")}
              onChange={(id, field, value) => updateRequestOverrideRow("body", id, field, value)}
              onRemove={(id) => removeRequestOverrideRow("body", id)}
            />
          </div>
          {!props.readOnly && <div className="flex justify-end">
            <button type="submit" className={buttonPrimary} disabled={props.saving}>
              {props.saving ? <Loader2 className={spinnerClass} size={18} /> : <Save size={18} />}
              保存覆盖
            </button>
          </div>}
        </form>
      </div>
    </Modal>
  );
}
