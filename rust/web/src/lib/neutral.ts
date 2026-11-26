/**
 * The console names no third-party component (GPL-3.0 section 7(e)): it says what the co-op splitter
 * does. The host's reasons and errors still name it, so text from the host passes through `neutral`
 * before the console shows it. A folder or file in a path stays as it is on the disk.
 */

const NAME = /Nucleus[ -]?Co-?op(\.exe)?/gi;
/** The word before already is an article: "a … session", "the … folder", "no … handler". */
const DETERMINER = /(?:^|[^\p{L}])(?:a|an|the|no|this|that|its|another|every|each)\s$/iu;
/** Used as an adjective: "… sessions need the service" reads as "co-op sessions". */
const ADJECTIVAL = /^\s(?:sessions?|handlers?|streams?|instances?)(?![\p{L}])/iu;
const SENTENCE_START = /(?:^|[.!?…]\s+)$/;

export function neutral(text: string): string {
  return text.replace(NAME, (match: string, exe: string | undefined, at: number) => {
    const before = text.slice(0, at);
    if (/[\\/]$/.test(before)) return match;
    const adjectival = !exe && ADJECTIVAL.test(text.slice(at + match.length));
    const noun = exe ? 'co-op splitter program' : adjectival ? 'co-op' : 'co-op splitter';
    const words = adjectival || DETERMINER.test(before) ? noun : `the ${noun}`;
    return SENTENCE_START.test(before) ? words.charAt(0).toUpperCase() + words.slice(1) : words;
  });
}
