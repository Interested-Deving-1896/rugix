import { ChevronRight } from "lucide-react";
import type { api } from "../../generated";
import { EmptyState } from "../../shared/components/EmptyState";
import { classes } from "../../shared/lib/classes";
import { generationLabel } from "../../shared/lib/format";
import { AppStatusBadge } from "../../shared/status/AppStatusBadge";

export function AppInventory({
  apps: appSummaries,
  selected,
  onSelect,
}: {
  apps: api.AppSummary[];
  selected?: string;
  onSelect: (app: string) => void;
}) {
  return (
    <div>
      <div className="hidden grid-cols-[minmax(0,1.4fr)_120px_120px_120px_24px] gap-3 border-b border-divider bg-elevation-2 px-4 py-2 text-xs font-semibold uppercase tracking-wide text-foreground-subtle md:grid">
        <div>App</div>
        <div>Workload</div>
        <div>Generation</div>
        <div>Scope</div>
        <div />
      </div>
      <div className="divide-y divide-divider">
        {appSummaries.map((app) => (
          <button
            key={app.name}
            className={classes(
              "grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-4 py-3 text-left transition hover:bg-elevation-2 md:grid-cols-[minmax(0,1.4fr)_120px_120px_120px_24px]",
              selected === app.name && "bg-primary-muted",
            )}
            onClick={() => onSelect(app.name)}
          >
            <span className="min-w-0">
              <span className="block truncate text-sm font-medium">{app.name}</span>
              <span className="mt-1 block text-xs text-foreground-muted md:hidden">
                generation {generationLabel(app.generation)}
              </span>
            </span>
            <span className="hidden md:block">
              <AppStatusBadge status={app.status} />
            </span>
            <span className="hidden font-mono text-sm text-foreground-muted md:block">{generationLabel(app.generation)}</span>
            <span className="hidden text-sm text-foreground-muted md:block">local</span>
            <span className="flex items-center gap-2 md:justify-end">
              <span className="md:hidden">
                <AppStatusBadge status={app.status} />
              </span>
              <ChevronRight size={16} className="text-foreground-subtle" />
            </span>
          </button>
        ))}
        {appSummaries.length === 0 && <EmptyState label="No apps installed." />}
      </div>
    </div>
  );
}
