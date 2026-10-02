// Feature-scoped translations keep the rich-text renderer independent of the large app dictionary.
export const richTextLabels = (vi: boolean) => vi ? {
  document: 'Document', email: 'Email draft', recipient: 'Recipient', copy: 'Copy', copied: 'Copied',
  copyFailed: 'Could not copy. Select and copy the content manually.', code: 'Code', source: 'Source',
  fileSource: 'File source', missingSource: 'Only reference IDs are available; this transcript has no source URLs.',
  widget: 'ChatGPT interactive content', missingWidget: 'View the original on ChatGPT; widget data is not included in this transcript.',
  unavailableLink: 'This transcript has no usable ChatCMD link.', unavailableImage: 'Image URL unavailable',
  note: 'Note', tip: 'Tip', important: 'Important', warning: 'Warning', caution: 'Caution',
  images: 'Images', links: 'Reference links', files: 'Reference files', products: 'Products', finance: 'Financial chart',
  weather: 'Weather', sports: 'Sports', video: 'Video', audio: 'Audio', map: 'Map', table: 'Data table', chart: 'Chart',
} : {
  document: 'Document', email: 'Email draft', recipient: 'Recipient', copy: 'Copy', copied: 'Copied',
  copyFailed: 'Could not copy. Select and copy the content manually.', code: 'Code', source: 'Source',
  fileSource: 'File source', missingSource: 'Only reference IDs are available; this transcript has no source URLs.',
  widget: 'ChatGPT interactive content', missingWidget: 'View the original on ChatGPT; widget data is not included in this transcript.',
  unavailableLink: 'This transcript has no usable ChatCMD link.', unavailableImage: 'Image URL unavailable',
  note: 'Note', tip: 'Tip', important: 'Important', warning: 'Warning', caution: 'Caution',
  images: 'Images', links: 'Reference links', files: 'Reference files', products: 'Products', finance: 'Financial chart',
  weather: 'Weather', sports: 'Sports', video: 'Video', audio: 'Audio', map: 'Map', table: 'Data table', chart: 'Chart',
};
export type RichTextLabels = ReturnType<typeof richTextLabels>;
