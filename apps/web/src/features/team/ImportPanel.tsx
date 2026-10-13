import { useState } from 'react';
import { Button, Card, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { plural } from '../../lib/format';
import type { ImportReport } from './types';

type Importer = { mutate: (v: { csv: string; dry_run: boolean }, o?: { onSuccess?: (r: ImportReport) => void }) => void; isPending: boolean; error: unknown };

/** A CSV box with "Check" (dry run, nothing saved) and "Import", and the row-by-row report. */
export function ImportPanel({ importer, columns, label }: { importer: Importer; columns: string; label: string }) {
  const [csv, setCsv] = useState('');
  const [report, setReport] = useState<ImportReport>();
  const run = (dry_run: boolean) => importer.mutate({ csv, dry_run }, { onSuccess: setReport });

  return (
    <Card className="flex flex-col gap-3">
      <p className="text-sm text-ink-muted">
        Paste a CSV with the columns <code>{columns}</code>.
      </p>
      <Textarea aria-label={label} rows={6} value={csv} onChange={(e) => setCsv(e.target.value)} className="font-mono text-xs" />
      <div className="flex gap-2">
        <Button variant="secondary" disabled={!csv.trim() || importer.isPending} onClick={() => run(true)}>
          Check only
        </Button>
        <Button disabled={!csv.trim() || importer.isPending} loading={importer.isPending} onClick={() => run(false)}>
          Import
        </Button>
      </div>
      <ErrorNote error={importer.error} />
      {report && (
        <div role="status" className="flex flex-col gap-1 text-sm">
          <p className="font-semibold text-ink">
            {report.dry_run ? 'Checked, nothing saved: ' : 'Imported: '}
            {plural(report.created, 'new row')}, {plural(report.updated, 'updated row')}, {plural(report.skipped, 'skipped row')}.
          </p>
          {report.errors.length > 0 && (
            <ul className="list-disc pl-5 text-danger">
              {report.errors.map((e) => (
                <li key={`${e.row}-${e.field}`}>
                  Row {e.row}, {e.field}: {e.message}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Card>
  );
}
