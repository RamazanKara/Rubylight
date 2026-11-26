/**
 * The letters a game without a cover shows in its place, the same in the console and on the play
 * page: "Half-Life 2" is "HL", "Hades II" is "H2".
 */

/** A sequel's roman numeral, read as its number. */
const ROMAN: Record<string, string> = { II: '2', III: '3', IV: '4', V: '5', VI: '6', VII: '7', VIII: '8', IX: '9', X: '10' };

export function initials(name: string): string {
  const words = name.trim().split(/[\s\-_:]+/).filter(Boolean);
  return words
    .slice(0, 2)
    // After the first word a numeral is a number, not a word: "Diablo IV" is D4, not DI.
    .map((word, index) => (index > 0 && ROMAN[word]) || (Array.from(word)[0] ?? ''))
    .join('')
    .toUpperCase();
}
