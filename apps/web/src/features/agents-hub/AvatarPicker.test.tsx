import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { agent } from '../../test/fixtures';
import { AvatarPicker } from './AvatarPicker';
import { AVATARS, parseAvatar, resolveAvatar } from './avatars';

describe('AvatarPicker', () => {
  it('offers every avatar as a radio and reports the stored value on click', async () => {
    const onChange = vi.fn();
    render(<AvatarPicker label="Choose" value="owl" onChange={onChange} />);
    expect(screen.getAllByRole('radio')).toHaveLength(AVATARS.length);
    expect(screen.getByRole('radio', { name: 'Owl' })).toBeChecked();
    await userEvent.click(screen.getByRole('radio', { name: 'Fox' }));
    expect(onChange).toHaveBeenCalledWith('fluent:fox');
  });

  it('moves with the arrow keys and only the selected radio is a tab stop', async () => {
    const onChange = vi.fn();
    render(<AvatarPicker label="Choose" value="owl" onChange={onChange} />);
    screen.getByRole('radio', { name: 'Owl' }).focus();
    await userEvent.keyboard('{ArrowRight}');
    expect(onChange).toHaveBeenCalledWith('fluent:bee');
    expect(screen.getByRole('radio', { name: 'Owl' })).toHaveAttribute('tabindex', '0');
    expect(screen.getByRole('radio', { name: 'Bee' })).toHaveAttribute('tabindex', '-1');
  });

  it('does nothing when disabled', async () => {
    const onChange = vi.fn();
    render(<AvatarPicker label="Choose" value="owl" disabled onChange={onChange} />);
    await userEvent.click(screen.getByRole('radio', { name: 'Fox' }));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('resolveAvatar', () => {
  it('uses the picked avatar, then the template default, then the category', () => {
    expect(parseAvatar('fluent:owl')).toBe('owl');
    expect(parseAvatar('fluent:nope')).toBeUndefined();
    expect(resolveAvatar(agent({ id: '1', name: 'x', category: 'finance', avatar: 'fluent:fox' }))).toBe('fox');
    expect(resolveAvatar(agent({ id: '2', name: 'x', category: 'general', kind: 'lead' }))).toBe('panda');
    expect(resolveAvatar(agent({ id: '3', name: 'x', category: 'finance', template_key: 'finance.invoices' }))).toBe('owl');
    expect(resolveAvatar(agent({ id: '4', name: 'x', category: 'sales' }))).toBe('bee');
    expect(resolveAvatar(agent({ id: '5', name: 'x', category: 'custom' }))).toBe('robot');
  });
});
