import type { api } from "../../generated";
import { Badge } from "../components/Badge";

export function GenerationStatusBadge({ generation }: { generation: api.AppGeneration }) {
  const label = generation.active ? "active" : generation.complete ? "ready" : "incomplete";
  const color = generation.active
    ? "bg-info-surface text-info ring-info/30"
    : generation.complete
      ? "bg-success-surface text-success ring-success/30"
      : "bg-warning-surface text-warning ring-warning/30";
  return <Badge color={color}>{label}</Badge>;
}
