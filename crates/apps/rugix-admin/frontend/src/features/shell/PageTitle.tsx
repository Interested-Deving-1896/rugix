import type { Tab } from "../../types";

export function PageTitle({ tab }: { tab: Tab }) {
  const meta = {
    system: { eyebrow: "Device", title: "System" },
    components: { eyebrow: "Compatibility", title: "Components" },
    apps: { eyebrow: "Runtime", title: "Apps" },
    jobs: { eyebrow: "Operations", title: "Jobs" },
  }[tab];
  return (
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div>
        <div className="text-xs font-semibold uppercase tracking-wide text-primary">{meta.eyebrow}</div>
        <h1 className="mt-1 font-display text-2xl font-semibold text-foreground">{meta.title}</h1>
      </div>
    </div>
  );
}
