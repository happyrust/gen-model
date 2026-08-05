/* AVEVA core.dll teach — shared quiz widget.
   Markup contract:
     <div class="quiz">
       <p class="q">question</p>
       <button data-correct="0|1" data-fb="feedback">answer</button>...
       <div class="fb"></div>
     </div>
   Retrieval practice depends on the learner committing before seeing feedback,
   so answers stay clickable after a wrong pick instead of locking the question. */
document.querySelectorAll('.quiz').forEach(function (q) {
  var fb = q.querySelector('.fb');
  q.querySelectorAll('button').forEach(function (b) {
    b.addEventListener('click', function () {
      q.querySelectorAll('button').forEach(function (x) {
        x.classList.remove('correct', 'wrong');
      });
      b.classList.add(b.dataset.correct === '1' ? 'correct' : 'wrong');
      if (fb) fb.textContent = b.dataset.fb || '';
    });
  });
});
