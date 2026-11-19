- An activation token is used only by a window of the seat it was asked on: a window of another
  seat that names it, through xdg-activation or an X11 window's startup id, is treated as having
  no valid token and leaves it unused, so that it can no longer stop the launched program from
  taking the focus (ADR-0026).
