import type { ReactNode } from "react";

export function ActionGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div>
      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-foreground-subtle">{title}</div>
      <div className="flex flex-wrap gap-2">{children}</div>
    </div>
  );
}
