(() => {
  function normalize(value) {
    return String(value || '').replace(/\s+/g, ' ').trim().toLowerCase();
  }

  function isVisible(element) {
    if (!(element instanceof Element)) return false;
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }

  function findVisible(selectors) {
    return findVisibleWithin(document, selectors);
  }

  function findVisibleWithin(root, selectors) {
    for (const selector of selectors) {
      for (const element of root.querySelectorAll(selector)) if (isVisible(element)) return element;
    }
    return null;
  }

  function assistantNodes() {
    return [...document.querySelectorAll('[data-message-author-role="assistant"]')].filter(isVisible);
  }

  function latestMessageText(role) {
    const nodes = [...document.querySelectorAll(`[data-message-author-role="${role}"]`)].filter(isVisible);
    const latest = nodes.at(-1);
    return latest?.innerText?.trim() || latest?.textContent?.trim() || '';
  }

  function findThreadError() {
    const latestUser = [...document.querySelectorAll('[data-message-author-role="user"]')].filter(isVisible).at(-1);
    const latestAssistant = assistantNodes().at(-1);
    const candidates = [...document.querySelectorAll([
      'button[data-testid="regenerate-thread-error-button"]',
      '[class*="text-token-text-error"]',
      '[class*="bg-token-surface-error"]',
      '[class*="border-token-surface-error"]',
    ].join(','))].filter(isVisible);
    return candidates.reverse().find((element) => {
      const afterLatestUser = !latestUser || Boolean(latestUser.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING);
      if (!afterLatestUser) return false;
      if (!latestAssistant) return true;
      const assistantAfterError = Boolean(element.compareDocumentPosition(latestAssistant) & Node.DOCUMENT_POSITION_FOLLOWING);
      return !assistantAfterError;
    }) || null;
  }

  function findStopButton() {
    const composers = [...document.querySelectorAll('form[data-type="unified-composer"]')].filter(isVisible);
    const root = composers.at(-1) || document;
    const direct = findVisibleWithin(root, [
      'button[data-testid="stop-button"]',
      'button[data-testid="stop-generating-button"]',
      'button[aria-label*="Stop" i]',
      'button[aria-label*="Stop" i]',
      'button[aria-label*="Stop" i]',
    ]);
    if (direct) return direct;
    return [...root.querySelectorAll('button')].filter(isVisible).find((button) =>
      /^(stop|stop|stop)(?:\s|$)/i.test(normalize(button.getAttribute('aria-label') || button.textContent))) || null;
  }

  function clickStopButton() {
    const button = findStopButton();
    if (button) button.click();
  }

  function findSendButton() {
    const composers = [...document.querySelectorAll('form[data-type="unified-composer"]')].filter(isVisible);
    const root = composers.at(-1) || document;
    const direct = findVisibleWithin(root, [
      'button[data-testid="send-button"]',
      'button[data-testid="composer-send-button"]',
      'button[data-testid="composer-submit-button"]',
      'button[aria-label="Send prompt" i]',
      'button[aria-label="Send message" i]',
      'button[aria-label="Send" i]',
      'button[aria-label*="send" i]',
      'button[aria-label*="보내" i]',
      'button[type="submit"]',
    ]);
    if (direct) return direct;
    return [...root.querySelectorAll('button,[role="button"]')].filter(isVisible).find((button) => {
      const label = normalize(button.getAttribute('aria-label') || button.getAttribute('title') || button.textContent);
      return /^(?:send(?: prompt| message)?|보내기|메시지 보내기|프롬프트 보내기)$/i.test(label);
    }) || null;
  }

  globalThis.ChatCmdConversationDom = Object.freeze({
    assistantNodes,
    clickStopButton,
    findSendButton,
    findStopButton,
    findThreadError,
    findVisible,
    isVisible,
    latestMessageText,
    normalize,
  });
})();
