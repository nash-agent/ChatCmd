import { describe, expect, it } from 'vitest';
import { prepareChatGptMessage } from './messageAttachments';

describe('prepareChatGptMessage', () => {
  it('sends the raw trimmed message when nothing is attached', () => {
    expect(prepareChatGptMessage('  Hello  ', {})).toBe('Hello');
  });

  it('adds only the selected plugin', () => {
    expect(prepareChatGptMessage('Do this work', { pluginName: 'rust_test' }))
      .toBe('plugin @rust_test\n\nrequest: Do this work');
  });

  it('adds only the selected project folder', () => {
    expect(prepareChatGptMessage('Do this work', { projectFolder: ' D:\\DEV\\CmdGPT ' }))
      .toBe('Project folder: D:\\DEV\\CmdGPT\n\nrequest: Do this work');
  });

  it('adds plugin and project in the requested order', () => {
    expect(prepareChatGptMessage('Do this work', { pluginName: 'rust_test', projectFolder: 'D:\\DEV\\CmdGPT' }))
      .toBe('plugin @rust_test\nProject folder: D:\\DEV\\CmdGPT\n\nrequest: Do this work');
  });
});
