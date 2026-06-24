import { Clock3 } from "lucide-react";
import type { jobs } from "../../generated";
import { EmptyState } from "../../shared/components/EmptyState";
import { Surface } from "../../shared/components/Surface";
import { classes } from "../../shared/lib/classes";
import { compactTime } from "../../shared/lib/format";
import { JobStatusBadge } from "../../shared/status/JobStatusBadge";
import type { JobLog } from "../../types";
import { JobLogSurface } from "./JobLogSurface";

export function JobsPage({
  jobs,
  selected,
  selectedJob,
  log,
  onSelect,
}: {
  jobs: jobs.Job[];
  selected?: string;
  selectedJob?: jobs.Job;
  log?: JobLog;
  onSelect: (id: string) => void;
}) {
  const orderedJobs = [...jobs].sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt));

  return (
    <div className="grid gap-5 xl:grid-cols-[minmax(300px,440px)_minmax(0,1fr)]">
      <Surface title="Recent Jobs" bodyClassName="p-0">
        <div className="divide-y divide-divider">
          {orderedJobs.map((job) => (
            <button
              key={job.id}
              onClick={() => onSelect(job.id)}
              className={classes(
                "group flex w-full items-center justify-between gap-4 px-4 py-3 text-left transition hover:bg-elevation-2",
                selected === job.id && "bg-primary-muted",
              )}
            >
              <span className="min-w-0">
                <span className="block truncate text-sm font-medium">{job.title}</span>
                <span className="mt-1 flex items-center gap-2 text-xs text-foreground-muted">
                  <Clock3 size={13} /> {compactTime(job.createdAt)}
                </span>
              </span>
              <JobStatusBadge status={job.status} />
            </button>
          ))}
          {orderedJobs.length === 0 && <EmptyState label="No jobs." />}
        </div>
      </Surface>

      <JobLogSurface jobId={selected} job={selectedJob} log={log} />
    </div>
  );
}
