--- blink.cmp source: complete chora `index.md` security levels.
--- Wired in nixvim via extraFiles + `sources.providers."chora-index"`.
--- @module 'blink.cmp'
--- @class blink.cmp.Source
local source = {}

local LEVELS = { 'open', 'restricted', 'critical' }

function source.new(_opts)
  return setmetatable({}, { __index = source })
end

--- Only on a `security:` line inside chora's index.md.
function source:enabled()
  if vim.bo.filetype ~= 'markdown' then
    return false
  end
  if vim.fn.expand('%:t') ~= 'index.md' then
    return false
  end
  local line = vim.api.nvim_get_current_line()
  return line:match('^%s*security:%s*%S*$') ~= nil
end

function source:get_trigger_characters()
  return { ':', ' ' }
end

function source:get_completions(_ctx, callback)
  local items = {}
  for _, level in ipairs(LEVELS) do
    table.insert(items, {
      label = level,
      kind = require('blink.cmp.types').CompletionItemKind.EnumMember,
      documentation = 'chora index.md security level',
    })
  end
  callback({ items = items, is_incomplete_backward = false, is_incomplete_forward = false })
  return function() end
end

return source
