import { ChevronDown, Upload } from "lucide-react";
import { useState, type ReactNode } from "react";
import type { InstallOptions } from "../../api";
import { Input } from "../../shared/components/Input";
import { Surface } from "../../shared/components/Surface";
import { formatBytes } from "../../shared/lib/format";
import { fieldClass, primaryButtonClass } from "../../shared/styles";

export function UploadPanel({
  title,
  fileLabel,
  icon,
  system,
  onUpload,
}: {
  title: string;
  fileLabel: string;
  icon: ReactNode;
  system?: boolean;
  onUpload: (file: File, options: InstallOptions) => void;
}) {
  const [file, setFile] = useState<File>();
  const [bundleHash, setBundleHash] = useState("");
  const [rootCert, setRootCert] = useState("");
  const [insecure, setInsecure] = useState(false);
  const [allowMissingIndex, setAllowMissingIndex] = useState(false);
  const [reboot, setReboot] = useState<InstallOptions["reboot"]>("no");

  return (
    <Surface title={title} icon={icon}>
      <div className="space-y-4">
        <label className="block">
          <input className="sr-only" type="file" onChange={(event) => setFile(event.target.files?.[0])} />
          <span className="flex min-h-24 cursor-pointer items-center justify-between gap-4 rounded-lg border border-dashed border-frame bg-elevation-0 px-4 py-3 transition hover:border-primary hover:bg-primary-muted">
            <span className="min-w-0">
              <span className="block text-sm font-medium text-foreground">{file?.name ?? fileLabel}</span>
              <span className="mt-1 block truncate text-xs text-foreground-muted">{file ? formatBytes(file.size) : "No file selected"}</span>
            </span>
            <span className="inline-flex h-9 shrink-0 items-center justify-center gap-2 rounded-md bg-primary px-3 text-sm font-medium text-primary-content">
              <Upload size={16} /> Choose
            </span>
          </span>
        </label>

        <details className="group border-t border-divider pt-3">
          <summary className="flex cursor-pointer list-none items-center justify-between text-sm font-medium text-foreground-muted transition hover:text-foreground">
            Advanced
            <ChevronDown size={16} className="transition group-open:rotate-180" />
          </summary>
          <div className="mt-3 grid gap-3 md:grid-cols-2">
            <Input label="Bundle hash" value={bundleHash} onChange={setBundleHash} />
            <Input label="Root certificate" value={rootCert} onChange={setRootCert} />
            {system && (
              <label className="block md:col-span-2">
                <span className="mb-1 block text-sm font-medium text-foreground-muted">Reboot</span>
                <select
                  className={fieldClass}
                  value={reboot}
                  onChange={(event) => setReboot(event.target.value as InstallOptions["reboot"])}
                >
                  <option value="no">No</option>
                  <option value="yes">Yes</option>
                  <option value="set">Set</option>
                  <option value="deferred">Deferred</option>
                </select>
              </label>
            )}
            <label className="inline-flex items-center gap-2 text-sm text-foreground-muted">
              <input className="size-4 accent-primary" type="checkbox" checked={insecure} onChange={(event) => setInsecure(event.target.checked)} />
              Skip verification
            </label>
            <label className="inline-flex items-center gap-2 text-sm text-foreground-muted">
              <input className="size-4 accent-primary" type="checkbox" checked={allowMissingIndex} onChange={(event) => setAllowMissingIndex(event.target.checked)} />
              Allow missing index
            </label>
          </div>
        </details>

        <div className="flex justify-end">
          <button
            className={primaryButtonClass}
            disabled={!file}
            onClick={() =>
              file &&
              onUpload(file, {
                bundleHash,
                rootCert,
                insecureSkipBundleVerification: insecure,
                insecureAllowMissingBlockIndex: allowMissingIndex,
                reboot: system ? reboot : undefined,
              })
            }
          >
            <Upload size={16} /> Install
          </button>
        </div>
      </div>
    </Surface>
  );
}
