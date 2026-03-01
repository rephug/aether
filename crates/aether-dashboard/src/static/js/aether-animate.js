(function () {
  function enterTransition(selection) {
    return selection
      .style('opacity', 0)
      .attr('transform', function () {
        const current = d3.select(this).attr('transform') || '';
        return `${current} scale(0.8)`;
      })
      .transition()
      .duration(600)
      .style('opacity', 1)
      .attr('transform', function () {
        return (d3.select(this).attr('transform') || '').replace(' scale(0.8)', '');
      });
  }

  function exitTransition(selection) {
    return selection
      .transition()
      .duration(600)
      .style('opacity', 0)
      .remove();
  }

  function pulseNode(selection) {
    selection
      .classed('aether-pulse', true)
      .transition()
      .duration(300)
      .on('end', function () {
        d3.select(this).classed('aether-pulse', false);
      });
  }

  window.AetherAnimate = {
    enterTransition,
    exitTransition,
    pulseNode,
  };
})();
