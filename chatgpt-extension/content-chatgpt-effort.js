(() => {
  const dom = globalThis.ChatCmdConversationDom;
  const clean = (value) => String(value || '').replace(/\s+/g, ' ').trim();
  const lower = (value) => clean(value).toLocaleLowerCase();

  function normalizeEffort(value) {
    const text = lower(value).replace(/[ _-]+/g, '');
    if (!text || ['inherit', 'current', 'keepcurrent', 'default', 'auto'].includes(text)) return 'inherit';
    if (['extrahigh', 'xhigh', 'max', 'maximum'].includes(text)) return 'extraHigh';
    if (text === 'high') return 'high';
    if (text === 'medium' || text === 'standard') return 'medium';
    if (text === 'low') return 'low';
    return '';
  }

  function effortFromText(value) {
    const text = lower(value);
    if (!text) return '';
    if (/\b(?:extra\s*high|x\s*high|maximum|max)\b/.test(text) || /(?:매우|아주)\s*높음|최고|최대/.test(text)) return 'extraHigh';
    if (/\bhigh\b/.test(text) || /높음/.test(text)) return 'high';
    if (/\b(?:medium|standard)\b/.test(text) || /중간|보통/.test(text)) return 'medium';
    if (/\blow\b/.test(text) || /낮음/.test(text)) return 'low';
    return '';
  }

  function descriptor(element) {
    if (!element) return '';
    return [
      element.textContent,
      element.getAttribute?.('aria-label'),
      element.getAttribute?.('title'),
      element.getAttribute?.('data-testid'),
    ].filter(Boolean).join(' ');
  }

  function selectedEffortFromDom() {
    const selectors = [
      '[role="menuitemradio"][aria-checked="true"]',
      '[role="radio"][aria-checked="true"]',
      '[role="option"][aria-selected="true"]',
      '[data-state="checked"]',
      '[aria-current="true"]',
    ];
    for (const selector of selectors) {
      for (const item of document.querySelectorAll(selector)) {
        const effort = effortFromText(descriptor(item));
        if (effort) return effort;
      }
    }
    return '';
  }

  function findEffortButton() {
    const selectors = [
      'button[data-testid*="reasoning" i]',
      'button[data-testid*="thinking" i]',
      'button[data-testid*="effort" i]',
      'button[aria-label*="reasoning" i]',
      'button[aria-label*="thinking" i]',
      'button[aria-label*="effort" i]',
      'button[aria-label*="추론"]',
      'button[aria-label*="생각"]',
    ];
    for (const selector of selectors) {
      for (const button of document.querySelectorAll(selector)) {
        if (dom.isVisible(button)) return button;
      }
    }
    for (const button of document.querySelectorAll('button[aria-haspopup="menu"], button[aria-haspopup="listbox"]')) {
      if (!dom.isVisible(button)) continue;
      const text = descriptor(button);
      if (effortFromText(text) || /reasoning|thinking|effort|추론|생각/i.test(text)) return button;
    }
    return null;
  }

  function currentEffort() {
    const selected = selectedEffortFromDom();
    if (selected) return selected;
    const button = findEffortButton();
    return button ? effortFromText(descriptor(button)) : '';
  }

  function visibleEffortOptions() {
    const selector = [
      '[role="menuitemradio"]',
      '[role="menuitem"]',
      '[role="option"]',
      '[data-radix-collection-item]',
      '[role="listbox"] button',
      '[role="menu"] button',
    ].join(',');
    return [...document.querySelectorAll(selector)]
      .filter((item) => dom.isVisible(item))
      .map((item) => ({ item, effort: effortFromText(descriptor(item)) }))
      .filter((entry) => entry.effort);
  }

  async function selectEffort(value) {
    const target = normalizeEffort(value);
    const before = currentEffort();
    if (!target || target === 'inherit') return { current: before || null, target: 'inherit', changed: false };
    if (before === target) return { current: before, target, changed: false };

    const button = findEffortButton();
    if (!button) throw new Error(`Could not find ChatGPT reasoning effort control for “${value}”.`);
    button.click();
    await waitForOptions();

    const options = visibleEffortOptions();
    const selected = options.find(({ item }) =>
      item.getAttribute?.('aria-checked') === 'true'
      || item.getAttribute?.('aria-selected') === 'true'
      || item.getAttribute?.('data-state') === 'checked'
    );
    if (selected?.effort === target) {
      button.click();
      return { current: target, target, changed: false };
    }

    const option = options.find((entry) => entry.effort === target)?.item;
    if (!option) {
      button.click();
      throw new Error(`Could not find reasoning effort “${value}” in the ChatGPT menu.`);
    }
    option.click();
    await new Promise((resolve) => setTimeout(resolve, 120));
    const after = currentEffort();
    if (after && after !== target) {
      throw new Error(`ChatGPT reasoning effort stayed at “${after}” instead of “${target}”.`);
    }
    return { current: after || target, target, changed: true };
  }

  async function waitForOptions() {
    const deadline = Date.now() + 4_000;
    while (Date.now() < deadline) {
      if (visibleEffortOptions().length) return;
      await new Promise((resolve) => setTimeout(resolve, 80));
    }
    throw new Error('ChatGPT reasoning effort menu did not open.');
  }

  globalThis.ChatCmdEffort = Object.freeze({
    normalize: normalizeEffort,
    current: currentEffort,
    select: selectEffort,
  });
})();
