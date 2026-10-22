// Javy の console（stdout／stderr → 本体のログ）。DOM の lib を持たない
// この SDK の型検査のため。使う側は DOM の lib で足りる。
declare var console: {
  log(...args: unknown[]): void;
  error(...args: unknown[]): void;
  warn(...args: unknown[]): void;
};

// Javy の TextEncoder／TextDecoder（UTF-8 だけ）。
declare class TextEncoder {
  encode(text?: string): Uint8Array;
}

declare class TextDecoder {
  constructor(label?: string);
  decode(bytes?: Uint8Array): string;
}

// dom.ts が定義する。
declare function setTimeout(fn: () => void, ms?: number): number;
