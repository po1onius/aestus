import { toast } from "sonner";
import { utf8ByteLength } from "../../lib/validation";
export function validPluginText(name: string, description: string) {
  if (!name.trim() || utf8ByteLength(name.trim()) > 128 || utf8ByteLength(description.trim()) > 1024) {
    toast.error("名称不能为空且最多 128 字节，备注最多 1024 字节。");
    return false;
  }
  return true;
}
