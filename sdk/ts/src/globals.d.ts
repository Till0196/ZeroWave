// Javy の console（stdout／stderr → 本体のログ）。DOM の lib を持たない
// この SDK の型検査のため。使う側は DOM の lib で足りる。
declare var console: {
  log(...args: unknown[]): void;
  error(...args: unknown[]): void;
  warn(...args: unknown[]): void;
};

