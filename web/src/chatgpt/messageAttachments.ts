export type ChatGptMessageAttachments = {
  pluginName?: string;
  projectFolder?: string;
};

export function prepareChatGptMessage(content: string, attachments: ChatGptMessageAttachments) {
  const message = content.trim();
  const pluginName = attachments.pluginName?.trim();
  const projectFolder = attachments.projectFolder?.trim();
  const context: string[] = [];
  if (pluginName) context.push(`plugin @${pluginName}`);
  if (projectFolder) context.push(`Project folder: ${projectFolder}`);
  return context.length ? `${context.join('\n')}\n\nrequest: ${message}` : message;
}
