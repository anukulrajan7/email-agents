import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import type { Attachment } from '../../api/types';
import { AttachmentChips } from './AttachmentChips';

const base: Attachment = { id: 'a1', name: 'file.bin', media_type: null, size_bytes: 2048, scan_status: 'clean' };

describe('AttachmentChips', () => {
  it('renders nothing without attachments', () => {
    const { container } = render(<AttachmentChips attachments={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('shows no AI state for files that have not been read', () => {
    render(<AttachmentChips attachments={[{ ...base, name: 'report.pdf' }]} />);
    expect(screen.getByText('report.pdf')).toBeInTheDocument();
    expect(screen.queryByText('Read by AI')).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('shows a sheet as a small table behind a preview toggle', async () => {
    const sheet: Attachment = {
      ...base,
      name: 'q3.xlsx',
      extract_kind: 'sheet',
      extract_status: 'done',
      preview: 'Sales: 2 rows × 2 columns',
      extract_meta: {
        summary: 'Sales: 2 rows × 2 columns',
        sheets: [{ name: 'Sales', rows: 2, columns: 2, truncated: false, sample: [['Region', 'Revenue'], ['Zanzibar', '1200'], ['Pune', '99']] }],
      },
    };
    render(<AttachmentChips attachments={[sheet]} />);
    expect(screen.getByText('Read by AI')).toBeInTheDocument();
    expect(screen.queryByRole('table')).toBeNull();

    const toggle = screen.getByRole('button', { name: 'Show preview' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await userEvent.click(toggle);

    expect(screen.getByRole('button', { name: 'Hide preview' })).toHaveAttribute('aria-expanded', 'true');
    const table = screen.getByRole('table', { name: 'Sales sample' });
    expect(within(table).getAllByRole('columnheader').map((h) => h.textContent)).toEqual(['Region', 'Revenue']);
    expect(within(table).getAllByRole('row').map((r) => r.textContent)).toEqual(['RegionRevenue', 'Zanzibar1200', 'Pune99']);
    expect(screen.getByText(/2 rows × 2 columns/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Hide preview' }));
    expect(screen.queryByRole('table')).toBeNull();
  });

  it('shows an image description, the text in it and its fields', async () => {
    const image: Attachment = {
      ...base,
      name: 'receipt.png',
      extract_kind: 'image',
      extract_status: 'done',
      preview: 'A cafe receipt',
      extract_meta: { summary: 'A cafe receipt', image_kind: 'receipt', text: 'Total 42 Mumbai Cafe', fields: { total: '42' } },
    };
    render(<AttachmentChips attachments={[image]} />);
    await userEvent.click(screen.getByRole('button', { name: 'Show preview' }));
    expect(screen.getByText('Receipt')).toBeInTheDocument();
    expect(screen.getByText('A cafe receipt')).toBeInTheDocument();
    expect(screen.getByText('Total 42 Mumbai Cafe')).toBeInTheDocument();
    expect(screen.getByText('total')).toBeInTheDocument();
    expect(screen.getByText('42')).toBeInTheDocument();
  });

  it('shows an audio summary with action items and says when the recording was cut short', async () => {
    const audio: Attachment = {
      ...base,
      name: 'voicemail.wav',
      extract_kind: 'audio',
      extract_status: 'done',
      preview: 'Caller wants the contract',
      extract_meta: { summary: 'Caller wants the contract', action_items: ['Send the contract', 'Call back Monday'], truncated: true },
    };
    render(<AttachmentChips attachments={[audio]} />);
    await userEvent.click(screen.getByRole('button', { name: 'Show preview' }));
    expect(screen.getByText('Caller wants the contract')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem').slice(-2).map((li) => li.textContent)).toEqual(['Send the contract', 'Call back Monday']);
    expect(screen.getByText('Only the first part of this recording was transcribed.')).toBeInTheDocument();
  });

  it('explains skipped and failed files without a preview, and stays quiet for unsupported types', () => {
    render(
      <AttachmentChips
        attachments={[
          { ...base, id: 'a1', name: 'old.png', extract_kind: 'image', extract_status: 'skipped', extract_note: 'Open this email to have AI read this file' },
          { ...base, id: 'a2', name: 'bad.png', extract_kind: 'image', extract_status: 'failed', extract_note: 'AI could not read this image' },
          { ...base, id: 'a3', name: 'legacy.doc', extract_kind: 'unsupported', extract_status: 'skipped', extract_note: 'This file type is not supported' },
        ]}
      />,
    );
    expect(screen.getByText('Open this email to have AI read this file')).toBeInTheDocument();
    expect(screen.getByText('AI could not read this image')).toBeInTheDocument();
    expect(screen.queryByText('This file type is not supported')).toBeNull();
    expect(screen.queryByText('Read by AI')).toBeNull();
  });
});
