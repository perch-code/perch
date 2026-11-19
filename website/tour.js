// Coordinates refer to the same 2760 × 1760 native window in both appearances.
// Drawer widths and popup rectangles follow the captures, so the site never
// has to reproduce a Perch control in HTML.
const views = {
  overview:    { left: .232, right: .244, showLeft: 1, showRight: 1, zoom: 1, x: .5, y: .5 },
  thread:      { left: .232, showLeft: 0, zoom: 1.34, x: .44, y: .49, cropAlign: 0 },
  code:        { right: .244, showRight: 1, zoom: 1.04, x: .54, y: .51, dockTree: true },
  review:      { left: .232, showLeft: 0, zoom: 1.28, x: .78, y: .26, cropAlign: 1, minViewWidth: .5,
                 popup: [1792, 86, 890, 1010], underlay: 'overview' },
  issue:       { left: .232, showLeft: 1, zoom: 2.18, x: .62, y: .18, minViewWidth: .37,
                 popup: [1300, 108, 810, 327], underlay: 'patch' },
  agents:      { zoom: 1.06, x: .57, y: .5, cropAlign: .35 },
  environment: { left: .232, showLeft: 1, zoom: 3.05, x: .13, y: .88, cropAlign: 0, minViewWidth: .26,
                 popup: [8, 1374, 555, 310], underlay: 'overview' },
};

const header = document.querySelector('.bar');
const tour = document.querySelector('.tour');
const steps = [...tour.querySelectorAll('.tour-step')];
const photos = steps.map(step => step.querySelector('img'));
const captions = steps.map(step => step.querySelector('.tour-copy'));
const stage = tour.querySelector('.tour-stage');
const captionStack = stage.querySelector('.tour-captions');
const frame = stage.querySelector('a');
const canvas = frame.querySelector('canvas');
const ctx = canvas.getContext('2d');
const dark = matchMedia('(prefers-color-scheme: dark)');
const reduced = matchMedia('(prefers-reduced-motion: reduce)');
const compact = matchMedia('(max-width: 48rem)');
const short = matchMedia('(max-height: 27rem)');
const clamp = n => Math.max(0, Math.min(1, n));
const mix = (a, b, t) => a + (b - a) * t;
const ease = t => { t = clamp(t); return t * t * (3 - 2 * t); };
const stateAt = i => views[steps[i].dataset.view];
const ready = img => img.complete && img.naturalWidth > 0;
const underlay = new Image();
const composite = document.createElement('canvas');
const brush = composite.getContext('2d');
let activeIndex = 0;
captions[activeIndex].classList.add('is-current');
let pending = false;

function schedule() {
  if (!pending) {
    pending = true;
    requestAnimationFrame(paint);
  }
}

function configure() {
  // Keep the direct link in sync for browsers without the in-page viewer.
  for (const [i, step] of steps.entries()) {
    const image = photos[i];
    step.querySelector('a.tour-still').href = dark.matches
      ? image.parentNode.querySelector('source').srcset : image.src;
  }
  const guided = Boolean(ctx && brush && ready(photos[0]) && !reduced.matches && !short.matches);
  tour.classList.toggle('is-guided', guided);
  stage.hidden = !guided;
  // On phones the actual captions share the pinned stage. Their articles
  // stay in the document as scroll stops and fragment-link destinations.
  const pinCaptions = guided && compact.matches;
  if (pinCaptions && !captionStack.hasChildNodes()) {
    captionStack.append(...captions);
  } else if (!pinCaptions && captionStack.hasChildNodes()) {
    steps.forEach((step, i) => step.prepend(captions[i]));
  }
  schedule();
}

function updateTheme() {
  underlay.src = 'img/tour/issue-underlay-' + (dark.matches ? 'dark' : 'light') + '.webp';
  configure();
}

// Use the untouched screenshot at rest. For the few frames when a popup is
// opening, paint the captured screen beneath it, then reveal the actual menu.
function imageWithPopup(img, state, amount) {
  if (!state.popup || amount >= 1) return img;
  const patch = state.underlay === 'patch' ? underlay : photos[0];
  if (!ready(patch)) return img;
  if (composite.width !== img.naturalWidth || composite.height !== img.naturalHeight) {
    composite.width = img.naturalWidth;
    composite.height = img.naturalHeight;
  }
  const [x, y, w, h] = state.popup;
  brush.drawImage(img, 0, 0);
  if (patch === underlay) brush.drawImage(patch, x, y, w, h);
  else brush.drawImage(patch, x, y, w, h, x, y, w, h);
  if (amount > 0) {
    brush.globalAlpha = amount;
    brush.drawImage(img, x, y, w, h, x, y + 14 * (1 - amount), w, h);
    brush.globalAlpha = 1;
  }
  return composite;
}

function drawWindow(index, leftOpen, rightOpen, popup, opacity, rightEdge) {
  const state = stateAt(index);
  const img = photos[index];
  const source = imageWithPopup(img, state, popup);
  const w = img.naturalWidth;
  const h = img.naturalHeight;
  const left = state.left || 0;
  const right = state.right || 0;
  const start = left * leftOpen;
  const end = 1 - right * rightOpen;
  // Expand the editor uniformly into the room freed by the two drawers.
  const scale = (end - start) / (1 - left - right);
  ctx.globalAlpha = opacity;
  ctx.drawImage(source, left*w, 0, (1-left-right)*w, h,
    start, (1-scale)*state.y, end-start, scale);
  if (start > 0) {
    ctx.drawImage(source, (left-start)*w, 0, start*w, h, 0, 0, start, 1);
  }
  if (end < 1) {
    // In a narrow code view, keep the tree beside the start of the code lines.
    const treeEnd = state.dockTree ? rightEdge : 1;
    ctx.drawImage(source, (1-right)*w, 0, (1-end)*w, h, treeEnd-(1-end), 0, 1-end, 1);
  }
}

function camera(state, aspect) {
  // Fit an open menu before cropping the sides for a portrait frame.
  const height = Math.min(1, Math.max(1/state.zoom, (state.minViewWidth || 0)/aspect));
  const width = height * aspect;
  const left = Math.min(1-height, Math.max(0, state.x-height/2));
  const crop = height-width;
  return {
    width, height,
    x: state.dockTree ? Math.max(0, left-crop) : left + crop * (state.cropAlign ?? .5),
    y: Math.min(1-height, Math.max(0, state.y-height/2)),
  };
}

function paint() {
  pending = false;
  if (stage.hidden) return;
  const stageBox = stage.getBoundingClientRect();
  // Outside the tour, leave the last rendered frame in place.
  if (stageBox.bottom < 0 || stageBox.top > innerHeight) return;
  const readingLine = compact.matches ? header.offsetHeight : stageBox.height/2;
  const stops = steps.map(step => {
    const box = step.getBoundingClientRect();
    return compact.matches ? box.top : box.top + box.height/2;
  });
  let index = 0;
  while (index < steps.length-1 && readingLine > stops[index+1]) index++;
  const next = Math.min(index+1, steps.length-1);
  const progress = index === next ? 0 : (readingLine-stops[index])/(stops[next]-stops[index]);
  // Hold each crop while its caption is in reading position.
  const t = ease((progress-.24)/.52);
  for (const photo of photos.slice(Math.max(0, index-1), next+2)) photo.loading = 'eager';
  if (!ready(photos[index]) || (t > 0 && !ready(photos[next]))) return;

  const density = Math.min(devicePixelRatio || 1, 3);
  const pixelWidth = Math.round(frame.clientWidth * density);
  const pixelHeight = Math.round(frame.clientHeight * density);
  if (!pixelWidth || !pixelHeight) return;
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  const current = stateAt(index), following = stateAt(next);
  const aspect = Math.min(1, (pixelWidth/pixelHeight) / (69/44));
  const a = camera(current, aspect), b = camera(following, aspect);
  const scaleX = pixelWidth / mix(a.width, b.width, t);
  const scaleY = pixelHeight / mix(a.height, b.height, t);
  ctx.setTransform(scaleX, 0, 0, scaleY, -mix(a.x,b.x,t)*scaleX, -mix(a.y,b.y,t)*scaleY);
  const left = mix(current.showLeft || 0, following.showLeft || 0, t);
  const right = mix(current.showRight || 0, following.showRight || 0, t);
  const rightEdge = compact.matches ? mix(a.x+a.width, b.x+b.width, t) : 1;
  const dissolve = ease((t-.20)/.56);
  if (dissolve < 1) drawWindow(index, left, right, 1-ease(t*3), 1, rightEdge);
  if (dissolve > 0) drawWindow(next, left, right, ease((t-.44)/.56), dissolve, rightEdge);
  ctx.globalAlpha = 1;

  const active = t < .5 ? index : next;
  if (active !== activeIndex) {
    captions[activeIndex].classList.remove('is-current');
    captions[active].classList.add('is-current');
    activeIndex = active;
  }
  frame.href = steps[active].querySelector('.tour-still').href;
  frame.setAttribute('aria-label', 'View full screenshot. ' + photos[active].alt);
}

const viewer = document.querySelector('.tour-viewer');
if (typeof viewer.showModal === 'function') {
  const holder = viewer.querySelector('.viewer-image');
  for (const trigger of tour.querySelectorAll('.tour-frame, .tour-still')) {
    trigger.setAttribute('aria-haspopup', 'dialog');
    trigger.addEventListener('click', event => {
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      const photo = trigger === frame ? photos[activeIndex] : trigger.querySelector('img');
      const picture = photo.closest('picture').cloneNode(true);
      picture.querySelector('img').loading = 'eager';
      holder.replaceChildren(picture);
      viewer.showModal();
    });
  }
  viewer.addEventListener('click', () => viewer.close());
  viewer.addEventListener('keydown', event => event.stopPropagation());
}

for (const photo of photos) photo.addEventListener('load', configure);
new ResizeObserver(schedule).observe(frame);
new ResizeObserver(schedule).observe(header, {box: 'border-box'});
underlay.addEventListener('load', schedule);
window.addEventListener('scroll', schedule, {passive: true});
window.addEventListener('resize', schedule);
dark.addEventListener('change', updateTheme);
for (const query of [compact, reduced, short]) query.addEventListener('change', configure);
document.fonts?.ready.then(schedule);
updateTheme();
