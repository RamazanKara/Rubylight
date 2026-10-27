const first = document.querySelector('.demo video');

if (first && first.dataset.posterPortrait && first.dataset.srcPortrait) {
  // The 16:9 tour is the default. Narrow screens get the 9:16 tour and its
  // poster. A fresh element is swapped in, so nothing is fetched before play,
  // and footage that has started is never replaced.
  const narrow = matchMedia('(max-width: 600px)');
  const fallback = first.querySelector('a');
  const landscape = { poster: first.getAttribute('poster'), src: first.querySelector('source').getAttribute('src') };
  const portrait = { poster: first.dataset.posterPortrait, src: first.dataset.srcPortrait };
  const make = footage => {
    const video = document.createElement('video');
    for (const { name, value } of first.attributes) if (name !== 'poster') video.setAttribute(name, value);
    video.setAttribute('poster', footage.poster);
    const source = document.createElement('source');
    source.setAttribute('src', footage.src);
    source.setAttribute('type', 'video/mp4');
    video.append(source);
    if (fallback) video.append(fallback.cloneNode(true));
    return video;
  };
  let current = first;
  const choose = () => {
    if (current.currentTime > 0 || !current.paused) return;
    const footage = narrow.matches ? portrait : landscape;
    if (current.getAttribute('poster') === footage.poster) return;
    const next = make(footage);
    current.replaceWith(next);
    current = next;
  };
  choose();
  narrow.addEventListener('change', choose);
}
