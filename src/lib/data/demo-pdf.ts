const pdfPage = (contentObject: number) =>
  `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents ${contentObject} 0 R >>`;

const pdfStream = (lines: readonly string[]) => {
  const stream = [
    "BT",
    "/F1 28 Tf",
    "72 690 Td",
    `(${lines[0]}) Tj`,
    "/F1 14 Tf",
    "0 -42 Td",
    `(${lines[1]}) Tj`,
    "0 -24 Td",
    `(${lines[2]}) Tj`,
    "ET",
  ].join("\n");
  return `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`;
};

export const createDemoPdf = (): ArrayBuffer => {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Count 3 /Kids [3 0 R 5 0 R 7 0 R] >>",
    pdfPage(4),
    pdfStream([
      "Explora PDF Preview",
      "A quiet, custom canvas viewer.",
      "Scroll to continue.",
    ]),
    pdfPage(6),
    pdfStream([
      "Local and direct",
      "Original PDF bytes stay behind an opaque reference.",
      "The WebView renders them with PDF.js.",
    ]),
    pdfPage(8),
    pdfStream([
      "Ready for the next page",
      "Thumbnails, zoom, and continuous navigation.",
      "No generic viewer chrome.",
    ]),
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];

  let source = "%PDF-1.7\n";
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(source.length);
    source += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xrefOffset = source.length;
  source += `xref\n0 ${objects.length + 1}\n`;
  source += "0000000000 65535 f \n";
  source += offsets
    .slice(1)
    .map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`)
    .join("");
  source += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\n`;
  source += `startxref\n${xrefOffset}\n%%EOF\n`;

  const bytes = new TextEncoder().encode(source);
  return bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;
};
