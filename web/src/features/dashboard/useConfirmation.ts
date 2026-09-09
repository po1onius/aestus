import { useState } from "react";

interface ConfirmationRequest {
  title: string;
  description: string;
  confirmLabel: string;
  pendingLabel: string;
  onConfirm: () => Promise<void>;
}

/** 确认框随所属页面销毁，不能把上一页面或会话的操作带到新页面。 */
export function useConfirmation() {
  const [confirmationRequest, setConfirmationRequest] = useState<ConfirmationRequest | null>(null);
  const [confirmationSubmitting, setConfirmationSubmitting] = useState(false);
  function closeConfirmationDialog() {
    if (!confirmationSubmitting) setConfirmationRequest(null);
  }
  async function confirmRequestedAction() {
    if (!confirmationRequest || confirmationSubmitting) return;
    setConfirmationSubmitting(true);
    try {
      await confirmationRequest.onConfirm();
    } finally {
      setConfirmationRequest(null);
      setConfirmationSubmitting(false);
    }
  }
  return { confirmationRequest, setConfirmationRequest, confirmationSubmitting, closeConfirmationDialog, confirmRequestedAction };
}
