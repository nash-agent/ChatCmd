import { FolderOpen, LoaderCircle } from 'lucide-react';
import { useId } from 'react';

import { tr, useAppLanguage } from '../i18n';

export function ProjectAccessScope({ checked, onChange, disabled, rootPath = '', onPickRoot, picking = false }: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled: boolean;
  rootPath?: string;
  onPickRoot?: () => void;
  picking?: boolean;
}) {
  const id = useId();
  useAppLanguage();
  return <div className="workspace-project-access">
    <label className="workspace-project-access-toggle">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} aria-describedby={`${id}-description ${id}-revoke`} />
      <span>{tr('Allow access in all conversations')}</span>
    </label>
    <p id={`${id}-description`}>{tr('When enabled and saved, the selected global root is accessible in all conversations. Existing tool permissions and approval requirements still apply.')}</p>
    {checked && <div className="workspace-project-access-root">
      <span>{tr('Global root')}</span>
      <button className={`workspace-project-folder ${rootPath ? '' : 'empty'}`} type="button" aria-label={tr('Global root')} onClick={onPickRoot} disabled={disabled || picking || !onPickRoot}>{picking ? <LoaderCircle className="spin" /> : <FolderOpen />}<span>{rootPath || tr('Choose global root')}</span></button>
      <p>{tr('Choose the folder shared with every conversation. A filesystem root such as D:\\ is allowed when you explicitly select it here.')}</p>
    </div>}
    <p id={`${id}-revoke`}>{tr("Uncheck and save, or delete this project, to revoke this project's shared access.")}</p>
  </div>;
}
