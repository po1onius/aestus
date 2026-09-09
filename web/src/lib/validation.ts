
export function utf8ByteLength(value: string) {
  return new TextEncoder().encode(value).byteLength;
}

export function isAscii(value: string) {
  return Array.from(value).every((character) => character.codePointAt(0)! <= 0x7f);
}
