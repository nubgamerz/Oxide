// Oxide's manual: the search box, and the words a search opened a page with highlighted.
//
// A classic script (Chrome refuses module scripts on file://), loaded after search-index.js,
// which sets window.OXIDE_SEARCH. The ranking is the generator's (crates/manual/src/search.rs,
// `query`): keep the two the same; tests/search_js.rs runs both on one index.
(function (root) {
  'use strict';

  var STOP_WORDS = ['a', 'an', 'and', 'are', 'as', 'at', 'be', 'by', 'can', 'do', 'does', 'for',
    'from', 'how', 'i', 'if', 'in', 'is', 'it', 'my', 'of', 'on', 'or', 'the', 'to', 'what',
    'when', 'where', 'which', 'why', 'with', 'you'];
  var TITLE = 30, TITLE_IN_SECTION = 3, HEADING = 10, TEXT = 1, EXACT = 2, PER_WORD = 1000,
    WHOLE_HEADING = 100;
  var WORD = /[\p{Alphabetic}\p{N}]+/gu;

  function words(text) {
    return (String(text).match(WORD) || []).map(function (w) { return w.toLowerCase(); });
  }

  function unique(list) {
    var seen = Object.create(null);
    return list.filter(function (w) {
      if (seen[w]) return false;
      seen[w] = true;
      return true;
    });
  }

  // The question's words, without the common ones (unless that leaves none).
  function question(text) {
    var all = unique(words(text));
    var kept = all.filter(function (w) { return STOP_WORDS.indexOf(w) < 0; });
    return kept.length ? kept : all;
  }

  function field(list, q) {
    var best = 0;
    for (var i = 0; i < list.length; i++) {
      if (list[i] === q) return EXACT;
      if (list[i].lastIndexOf(q, 0) === 0) best = 1;
    }
    return best;
  }

  // Every entry matching any word of `text`, best first: [{entry, score}].
  function search(index, text) {
    var q = question(text);
    if (!q.length || !index) return [];
    var hits = [];
    index.entries.forEach(function (e, n) {
      var titleWeight = e.anchor === '' ? TITLE : TITLE_IN_SECTION;
      var title = index.pages[e.page].words;
      var matched = 0, sum = 0, inHeading = 0;
      q.forEach(function (w) {
        var heading = field(e.hw, w);
        if (heading > 0) inHeading++;
        var best = Math.max(field(title, w) * titleWeight, heading * HEADING,
          field(e.tw, w) * TEXT);
        if (best > 0) { matched++; sum += best; }
      });
      // A section whose heading holds every word (manual-search's WHOLE_HEADING).
      if (e.anchor !== '' && q.length > 1 && inHeading === q.length) sum += WHOLE_HEADING;
      if (matched) hits.push({ entry: n, score: matched * PER_WORD + sum });
    });
    hits.sort(function (a, b) { return b.score - a.score || a.entry - b.entry; });
    return hits;
  }

  function escapeHtml(text) {
    return String(text).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  // Words starting with any of `q`, matched whole: group 2 is the word.
  function wordsPattern(q) {
    var alternatives = q.map(function (w) { return w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); });
    return new RegExp('(^|[^\\p{Alphabetic}\\p{N}])((?:' + alternatives.join('|') +
      ')[\\p{Alphabetic}\\p{N}]*)', 'giu');
  }

  // `text` as HTML with the words matching `pattern` marked.
  function markHtml(text, pattern) {
    var out = '', last = 0, m;
    pattern.lastIndex = 0;
    while ((m = pattern.exec(text))) {
      var start = m.index + m[1].length;
      out += escapeHtml(text.slice(last, start)) + '<mark>' + escapeHtml(m[2]) + '</mark>';
      last = start + m[2].length;
      if (m[0].length === 0) pattern.lastIndex++;
    }
    return out + escapeHtml(text.slice(last));
  }

  // A short piece of `text` around the first matching word, as HTML with the words marked.
  function snippet(text, q, length) {
    length = length || 170;
    if (!text) return '';
    var pattern = wordsPattern(q);
    var m = pattern.exec(text);
    var at = m ? m.index + m[1].length : 0;
    var start = Math.max(0, at - 50);
    if (start > 0) {
      var space = text.indexOf(' ', start);
      start = space >= 0 && space < at ? space + 1 : start;
    }
    var end = Math.min(text.length, start + length);
    if (end < text.length) {
      var cut = text.lastIndexOf(' ', end);
      if (cut > at) end = cut;
    }
    return (start > 0 ? '…' : '') + markHtml(text.slice(start, end), wordsPattern(q)) +
      (end < text.length ? '…' : '');
  }

  root.OxideManual = { search: search, question: question, words: words, snippet: snippet };

  if (typeof document === 'undefined') return;

  // ---------------------------------------------------------------- the search box

  function setUpSearch() {
    var input = document.getElementById('ox-search');
    var list = document.getElementById('ox-results');
    var index = root.OXIDE_SEARCH;
    if (!input || !list) return;
    if (!index) {
      input.placeholder = 'Search unavailable (search-index.js is missing)';
      input.disabled = true;
      return;
    }
    var active = -1;
    var shown = [];

    function close() {
      list.hidden = true;
      list.innerHTML = '';
      input.setAttribute('aria-expanded', 'false');
      input.removeAttribute('aria-activedescendant');
      active = -1;
      shown = [];
    }

    function select(n) {
      var items = list.querySelectorAll('li[role="option"]');
      if (!items.length) return;
      active = (n + items.length) % items.length;
      for (var i = 0; i < items.length; i++) {
        items[i].setAttribute('aria-selected', i === active ? 'true' : 'false');
      }
      input.setAttribute('aria-activedescendant', items[active].id);
      items[active].scrollIntoView({ block: 'nearest' });
    }

    function update() {
      var text = input.value.trim();
      if (!text) { close(); return; }
      var q = question(text);
      var hits = search(index, text).slice(0, 12);
      shown = hits.map(function (h) {
        var e = index.entries[h.entry];
        var page = index.pages[e.page];
        var href = page.file + '?q=' + encodeURIComponent(text) + (e.anchor ? '#' + e.anchor : '');
        return { href: href, page: page, entry: e };
      });
      var html = shown.map(function (s, n) {
        var where = escapeHtml(s.page.title) + (s.entry.heading
          ? '<span class="ox-r-sep">›</span>' + escapeHtml(s.entry.heading) : '');
        return '<li role="option" id="ox-r-' + n + '" aria-selected="false"><a href="' +
          escapeHtml(s.href) + '"><span class="ox-r-where">' + where +
          '</span><span class="ox-r-snip">' + snippet(s.entry.text, q) + '</span></a></li>';
      }).join('');
      list.innerHTML = html ||
        '<li class="ox-none">No pages match “' + escapeHtml(text) + '”.</li>';
      list.hidden = false;
      input.setAttribute('aria-expanded', 'true');
      active = -1;
      if (shown.length) select(0);
    }

    input.addEventListener('input', update);
    input.addEventListener('focus', function () { if (input.value.trim()) update(); });
    input.addEventListener('keydown', function (ev) {
      if (ev.key === 'ArrowDown') { ev.preventDefault(); select(active + 1); }
      else if (ev.key === 'ArrowUp') { ev.preventDefault(); select(active - 1); }
      else if (ev.key === 'Enter') {
        ev.preventDefault();
        var pick = shown[active >= 0 ? active : 0];
        if (pick) root.location.href = pick.href;
      } else if (ev.key === 'Escape') {
        if (!list.hidden) close(); else { input.value = ''; input.blur(); }
      }
    });
    document.addEventListener('mousedown', function (ev) {
      if (!ev.target.closest || !ev.target.closest('.ox-search')) close();
    });
    document.addEventListener('keydown', function (ev) {
      var typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName);
      if ((ev.key === '/' && !typing) || (ev.key === 'k' && (ev.ctrlKey || ev.metaKey))) {
        ev.preventDefault();
        input.focus();
        input.select();
      }
    });
    return input;
  }

  // ---------------------------------------------------------------- the words a search opened

  function queryWords() {
    var match = /[?&]q=([^&#]*)/.exec(root.location.search);
    if (!match) return '';
    try { return decodeURIComponent(match[1].replace(/\+/g, ' ')); } catch (e) { return ''; }
  }

  function highlight(text) {
    var article = document.getElementById('ox-article');
    var q = question(text);
    if (!article || !q.length) return;
    var pattern = wordsPattern(q);
    var nodes = [];
    var walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT, {
      acceptNode: function (node) {
        var parent = node.parentNode;
        if (!node.nodeValue.trim() || parent.closest('script, style, .ox-anchor, mark')) {
          return NodeFilter.FILTER_REJECT;
        }
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    while (walker.nextNode()) nodes.push(walker.currentNode);
    var count = 0;
    nodes.forEach(function (node) {
      var value = node.nodeValue, last = 0, m, fragment = null;
      pattern.lastIndex = 0;
      while ((m = pattern.exec(value))) {
        var start = m.index + m[1].length;
        fragment = fragment || document.createDocumentFragment();
        fragment.appendChild(document.createTextNode(value.slice(last, start)));
        var mark = document.createElement('mark');
        mark.className = 'ox-hit';
        mark.textContent = m[2];
        fragment.appendChild(mark);
        count++;
        last = start + m[2].length;
      }
      if (fragment) {
        fragment.appendChild(document.createTextNode(value.slice(last)));
        node.parentNode.replaceChild(fragment, node);
      }
    });

    var bar = document.createElement('div');
    bar.className = 'ox-hits';
    bar.setAttribute('role', 'status');
    bar.innerHTML = '<span>' + (count
      ? count + ' match' + (count === 1 ? '' : 'es') + ' for <b>' + escapeHtml(q.join(' ')) + '</b> highlighted'
      : 'No matches for <b>' + escapeHtml(q.join(' ')) + '</b> on this page') +
      '</span><button type="button">Clear</button>';
    var title = article.querySelector('h1');
    article.insertBefore(bar, title ? title.nextSibling : article.firstChild);
    bar.querySelector('button').addEventListener('click', function () {
      var marks = article.querySelectorAll('mark.ox-hit');
      for (var i = 0; i < marks.length; i++) {
        var parent = marks[i].parentNode;
        parent.replaceChild(document.createTextNode(marks[i].textContent), marks[i]);
        parent.normalize();
      }
      bar.remove();
      // Drop ?q= from the address; a file:// page may refuse (its origin is opaque).
      try {
        root.history.replaceState(null, '', root.location.pathname + root.location.hash);
      } catch (e) { /* the marks are gone; the address keeps its ?q= */ }
    });

    if (!root.location.hash) {
      var first = article.querySelector('mark.ox-hit');
      if (first) first.scrollIntoView({ block: 'center' });
    }
  }

  function start() {
    var input = setUpSearch();
    var text = queryWords();
    if (text) {
      if (input) input.value = text;
      highlight(text);
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})(typeof window !== 'undefined' ? window : globalThis);
