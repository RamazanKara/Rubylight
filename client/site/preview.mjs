import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const prefix = '/nightjar';
const types = {
  '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.svg': 'image/svg+xml',
  '.png': 'image/png', '.webp': 'image/webp', '.ico': 'image/x-icon',
  '.mp4': 'video/mp4', '.json': 'application/json; charset=utf-8'
};

const server = createServer(async (request, response) => {
  try {
    if (!['GET', 'HEAD'].includes(request.method)) {
      response.writeHead(405, { Allow: 'GET, HEAD' }).end();
      return;
    }
    let pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
    if (pathname === prefix) {
      response.writeHead(308, { Location: prefix + '/' }).end();
      return;
    }
    if (pathname.startsWith(prefix + '/')) pathname = pathname.slice(prefix.length);
    const target = join(root, pathname);
    const local = relative(root, target);
    if (local.startsWith('..' + sep) || local === '..' || pathname.includes('\\') || pathname.includes('\0')) {
      response.writeHead(403).end();
      return;
    }
    let file = target;
    let info;
    try {
      info = await stat(file);
      if (info.isDirectory()) {
        if (!pathname.endsWith('/')) {
          response.writeHead(308, { Location: new URL(request.url, 'http://localhost').pathname + '/' }).end();
          return;
        }
        file = join(file, 'index.html');
        info = await stat(file);
      }
    } catch {
      const page = await readFile(join(root, '404.html'));
      response.writeHead(404, { 'Content-Type': types['.html'], 'Cache-Control': 'no-store' });
      response.end(request.method === 'HEAD' ? undefined : page);
      return;
    }
    const headers = {
      'Content-Type': types[extname(file)] || 'application/octet-stream',
      'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
      'Accept-Ranges': 'bytes'
    };
    let start = 0;
    let end = info.size - 1;
    let status = 200;
    if (request.headers.range) {
      const match = /^bytes=(\d*)-(\d*)$/.exec(request.headers.range);
      if (match) {
        start = match[1] ? Number(match[1]) : Math.max(0, info.size - Number(match[2]));
        end = match[1] && match[2] ? Math.min(Number(match[2]), end) : end;
      }
      if (!match || start > end || start >= info.size) {
        response.writeHead(416, { 'Content-Range': 'bytes */' + info.size }).end();
        return;
      }
      status = 206;
      headers['Content-Range'] = 'bytes ' + start + '-' + end + '/' + info.size;
    }
    headers['Content-Length'] = info.size ? end - start + 1 : 0;
    response.writeHead(status, headers);
    if (request.method === 'HEAD' || !info.size) response.end();
    else createReadStream(file, { start, end }).pipe(response);
  } catch {
    response.writeHead(400).end('Bad request');
  }
});

server.listen(4317, '127.0.0.1', () => {
  console.log('Nightjar preview: http://127.0.0.1:4317/nightjar/');
});
server.on('error', error => {
  console.error(error.message);
  process.exitCode = 1;
});
