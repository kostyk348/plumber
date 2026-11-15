" plumb.vim — отдать текущее место в plumber и не думать, какой редактор его откроет.
"
" Установка:
"   mkdir -p ~/.vim/pack/plumber/start/plumb/plugin
"   cp contrib/vim/plumb.vim ~/.vim/pack/plumber/start/plumb/plugin/
"
" Использование:
"   :Plumb            " текущий файл и строка
"   :Plumb <текст>    " произвольный текст (URL, номер детали, что угодно)
"   <leader>p         " то же, что :Plumb
"
" Правила решают, куда это поедет: в редактор (другой), в браузер, в просмотрщик PDF.
" Сам плагин ничего не знает про программы — только про `plumb send`.

if exists('g:loaded_plumb')
  finish
endif
let g:loaded_plumb = 1

function! s:Plumb(...) abort
  if a:0 == 0 || empty(a:1)
    let l:text = expand('%:p') . ':' . line('.')
  else
    let l:text = join(a:000, ' ')
  endif
  let l:cmd = get(g:, 'plumb_executable', 'plumb')
  let l:out = system(l:cmd . ' send ' . shellescape(l:text))
  let l:code = v:shell_error
  if l:code != 0
    echohl ErrorMsg | echomsg 'plumb: ' . substitute(l:out, '\n\+$', '', '') | echohl None
  else
    echomsg 'plumb: ' . substitute(l:out, '\n\+$', '', '')
  endif
  return ''
endfunction

command! -nargs=* Plumb call s:Plumb(<q-args>)
nnoremap <silent> <leader>p :<C-u>Plumb<CR>
