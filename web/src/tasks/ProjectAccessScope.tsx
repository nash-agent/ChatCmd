import { useId } from 'react';

import { tr, useAppLanguage } from '../i18n';

export function ProjectAccessScope({ checked, onChange, disabled }: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled: boolean;
}) {
  const id = useId();
  useAppLanguage();
  return <div className="workspace-project-access">
    <label className="workspace-project-access-toggle">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} aria-describedby={`${id}-description ${id}-revoke`} />
      <span>{tr('Allow access in all conversations')}</span>
    </label>
    <p id={`${id}-description`}>{tr('When enabled and saved, this registered folder is accessible in all conversations. Existing tool permissions and approval requirements still apply.')}</p>
    <p id={`${id}-revoke`}>{tr("Uncheck and save, or delete this project, to revoke this project's shared access.")}</p>
  </div>;
}
