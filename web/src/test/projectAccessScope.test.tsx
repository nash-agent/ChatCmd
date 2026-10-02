import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { api } from '../api';
import { setAppLanguage } from '../i18n';
import { ProjectAccessScope } from '../tasks/ProjectAccessScope';
import { TaskRail } from '../tasks/TaskRail';
import type { WorkspaceProject } from '../types';

vi.mock('../realtime', () => ({ useRealtime: vi.fn() }));

const project: WorkspaceProject = { id: 'project-1', name: 'Dotty', path: '/projects/dotty' };
const checkboxName = 'Allow access in all conversations';
const checkbox = () => screen.getByRole('checkbox', { name: checkboxName });

function backend(initial: WorkspaceProject[] = []) {
  let projects = [...initial];
  vi.spyOn(api, 'tasks').mockResolvedValue({ items: [] });
  vi.spyOn(api, 'workspaceProjects').mockImplementation(async () => projects);
  vi.spyOn(api, 'pickProjectFolder').mockResolvedValue({ path: project.path });
  vi.spyOn(api, 'saveWorkspaceProject').mockImplementation(async (input) => {
    const saved = { ...input, id: project.id };
    projects = [...projects, saved];
    return saved;
  });
  vi.spyOn(api, 'updateWorkspaceProject').mockImplementation(async (id, input) => {
    const saved = { ...input, id };
    projects = projects.map((current) => current.id === id ? saved : current);
    return saved;
  });
}

function renderRail() {
  return render(<MemoryRouter><TaskRail open onClose={vi.fn()} onDesktopCollapse={vi.fn()} /></MemoryRouter>);
}

async function addProject() {
  await userEvent.click(screen.getByRole('button', { name: 'Add project' }));
  await userEvent.type(screen.getByLabelText('Name'), project.name);
  await userEvent.click(screen.getByRole('button', { name: 'Project folder' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Project folder' })).toHaveTextContent(project.path));
}

async function editProject() {
  fireEvent.contextMenu(await screen.findByText(project.name));
  await userEvent.click(screen.getByRole('menuitem', { name: 'Edit project' }));
}

beforeEach(() => setAppLanguage('en', false));
afterEach(() => { vi.restoreAllMocks(); setAppLanguage('en', false); });

describe('project access scope', () => {
  it('keeps new projects unchecked and explicitly saves false', async () => {
    backend();
    renderRail();
    await addProject();
    expect(checkbox()).not.toBeChecked();
    expect(checkbox()).toHaveAccessibleDescription(/Existing tool permissions and approval requirements still apply/);
    expect(checkbox()).toHaveAccessibleDescription(/Uncheck and save, or delete this project/);
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.saveWorkspaceProject).toHaveBeenCalledWith({ name: project.name, path: project.path, chatGptProjectUrl: '', allowAllConversations: false });
  });

  it('persists an explicit opt-in and reloads the checked value when editing', async () => {
    backend();
    renderRail();
    await addProject();
    await userEvent.click(checkbox());
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.saveWorkspaceProject).toHaveBeenCalledWith(expect.objectContaining({ allowAllConversations: true }));
    await editProject();
    expect(checkbox()).toBeChecked();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await userEvent.click(screen.getByRole('button', { name: 'Add project' }));
    expect(checkbox()).not.toBeChecked();
  });

  it.each([undefined, false])('keeps existing scope %s unchecked on edit', async (allowAllConversations) => {
    backend([{ ...project, allowAllConversations }]);
    renderRail();
    await editProject();
    expect(checkbox()).not.toBeChecked();
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.updateWorkspaceProject).toHaveBeenCalledWith(project.id, expect.objectContaining({ allowAllConversations: false }));
  });

  it('revokes persisted shared access by saving an unchecked edit', async () => {
    backend([{ ...project, allowAllConversations: true }]);
    renderRail();
    await editProject();
    expect(checkbox()).toBeChecked();
    await userEvent.click(checkbox());
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.updateWorkspaceProject).toHaveBeenCalledWith(project.id, expect.objectContaining({ allowAllConversations: false }));
    await editProject();
    expect(checkbox()).not.toBeChecked();
  });

  it('uses the saved server response without risking a failed list refresh after revocation', async () => {
    backend([{ ...project, allowAllConversations: true }]);
    renderRail();
    await editProject();
    vi.mocked(api.workspaceProjects).mockRejectedValueOnce(new Error('Refresh unavailable.'));
    await userEvent.click(checkbox());
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.workspaceProjects).toHaveBeenCalledTimes(1);
    await editProject();
    expect(checkbox()).not.toBeChecked();
  });

  it.each(['Cancel', 'Close dialog', 'Escape'])('discards an unsaved scope change with %s', async (dismiss) => {
    backend([{ ...project, allowAllConversations: true }]);
    renderRail();
    await editProject();
    await userEvent.click(checkbox());
    if (dismiss === 'Escape') await userEvent.keyboard('{Escape}');
    else await userEvent.click(screen.getByRole('button', { name: dismiss }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(api.updateWorkspaceProject).not.toHaveBeenCalled();
    await editProject();
    expect(checkbox()).toBeChecked();
  });

  it('blocks repeated save and dismissal while saving, then retains a failed edit for retry', async () => {
    backend([{ ...project, allowAllConversations: true }]);
    let failSave!: (error: Error) => void;
    vi.mocked(api.updateWorkspaceProject).mockImplementationOnce(() => new Promise((_resolve, reject) => { failSave = reject; }));
    renderRail();
    await editProject();
    await userEvent.click(checkbox());
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(checkbox()).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Saving…' }));
    await userEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    await userEvent.keyboard('{Escape}');
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(api.updateWorkspaceProject).toHaveBeenCalledTimes(1);
    await act(async () => failSave(new Error('Could not save access.')));
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not save access.');
    expect(checkbox()).not.toBeChecked();
    expect(checkbox()).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.updateWorkspaceProject).toHaveBeenCalledTimes(2);
    await editProject();
    expect(checkbox()).not.toBeChecked();
  });

  it('does not show failed revocation as persisted after cancelling', async () => {
    backend([{ ...project, allowAllConversations: true }]);
    vi.mocked(api.updateWorkspaceProject).mockRejectedValueOnce(new Error('Permission update failed.'));
    renderRail();
    await editProject();
    await userEvent.click(checkbox());
    await userEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Permission update failed.');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await editProject();
    expect(checkbox()).toBeChecked();
  });

  it('translates the access label and policy explanation in Vietnamese', () => {
    setAppLanguage('vi', false);
    render(<ProjectAccessScope checked={false} disabled={false} onChange={vi.fn()} />);
    const control = screen.getByRole('checkbox', { name: 'Cho phép truy cập trong mọi cuộc trò chuyện' });
    expect(control).not.toBeChecked();
    expect(control).toHaveAccessibleDescription(/Quyền sử dụng công cụ và các yêu cầu phê duyệt hiện có vẫn được áp dụng/);
    expect(control).toHaveAccessibleDescription(/thu hồi quyền truy cập dùng chung/);
    act(() => setAppLanguage('en', false));
    expect(checkbox()).toBeInTheDocument();
  });
});
