const MAX_SEGMENT_CHARS = 180;

/** Extract complete sentences, with bounded segments for unpunctuated output. */
export function splitSpeechText(text: string, flush = false): { segments: string[]; remainder: string } {
  const segments: string[] = [];
  let remainder = text;
  while (remainder.trim()) {
    const boundary = /[。！？!?；;\n]/u.exec(remainder);
    let end = boundary ? boundary.index + boundary[0].length : 0;
    if (!end || end > MAX_SEGMENT_CHARS) {
      if (remainder.length < MAX_SEGMENT_CHARS && !flush) break;
      end = Math.min(remainder.length, MAX_SEGMENT_CHARS);
      const prefix = remainder.slice(0, end);
      const softBoundary = Math.max(prefix.lastIndexOf("，"), prefix.lastIndexOf(","), prefix.lastIndexOf(" "));
      if (softBoundary >= 24) end = softBoundary + 1;
      // Avoid splitting a UTF-16 surrogate pair at the segment boundary.
      const last = remainder.charCodeAt(end - 1);
      if (last >= 0xd800 && last <= 0xdbff) end -= 1;
    }
    const segment = remainder.slice(0, end).trim();
    if (segment) segments.push(segment);
    remainder = remainder.slice(end);
  }
  return { segments, remainder: remainder.trim() ? remainder : "" };
}
