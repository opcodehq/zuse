local s = ui.state { count = 0, enabled = false, scale = 2, tab = "Weekly" }
return function()
 return ui.column { gap = 12,
  ui.row { gap = 14,
   ui.stat { value = tostring(s.count), label = "Interactions" },
   ui.button { label = "Increment", primary = true, on_click = function() s.count += 1 end },
   ui.toggle { label = "Forecast", value = s.enabled, on_change = function(v) s.enabled = v end },
  },
  ui.tabs { options = { "Weekly", "Monthly" }, value = s.tab, on_change = function(v) s.tab = v end },
  ui.slider { label = "Scale", value = s.scale, min = 1, max = 5, step = 1, on_change = function(v) s.scale = v end },
  ui.chart { type = "bar", height = 140, x = { "Mon", "Tue", "Wed", "Thu" }, series = { { name = "Activity", values = { 2*s.scale, 3*s.scale, (s.count+1)*s.scale, 4*s.scale } } } },
  ui.canvas { height = 80, animate = false, on_click = function(x,y) s.count += 10 end, draw = function(g,w,h,t)
    g:line(0,h/2,w,h/2,{color="accent",width=2,dash=6})
    g:text(w/2,10,"Canvas interactions: "..tostring(s.count),{align="center"})
  end },
 }
end
