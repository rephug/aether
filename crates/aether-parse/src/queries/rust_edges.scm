(call_expression) @edge.call
(use_declaration) @edge.depends_on
(parameter type: (_) @edge.type_ref)
(function_item return_type: (_) @edge.type_ref)
(impl_item
  trait: (_) @trait
  type: (_) @self_type) @edge.implements
